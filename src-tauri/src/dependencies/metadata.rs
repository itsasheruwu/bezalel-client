//! Static inspection only: no Java, Gradle, entrypoints or third-party scripts are executed.
use crate::state::{err, Result, Target};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cmp::Ordering,
    io::{Cursor, Read},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Requirement {
    pub id: String,
    pub ranges: Vec<String>,
    pub kind: String,
    pub side: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mod {
    pub id: String,
    pub version: String,
    pub side: String,
    pub requirements: Vec<Requirement>,
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn entry(bytes: &[u8], name: &str) -> Result<Option<Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(err)?;
    let result = match zip.by_name(name) {
        Ok(mut f) => {
            if f.size() > 4 * 1024 * 1024 {
                return Err("Mod metadata or nested JAR exceeds inspection limit.".into());
            }
            let mut b = Vec::new();
            f.by_ref()
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut b)
                .map_err(err)?;
            Ok(Some(b))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(err(e)),
    };
    result
}
pub fn inspect(bytes: &[u8], target: Target) -> Result<Vec<Mod>> {
    inspect_depth(bytes, target, 0)
}
fn inspect_depth(bytes: &[u8], target: Target, depth: usize) -> Result<Vec<Mod>> {
    if bytes.len() > 64 * 1024 * 1024 || depth > 8 {
        return Err("Mod inspection limit exceeded.".into());
    }
    let mut out = Vec::new();
    if target.loader == "fabric" {
        let raw = entry(bytes, "fabric.mod.json")?
            .ok_or("This JAR has no Fabric runtime metadata for the selected loader.")?;
        let v: Value = serde_json::from_slice(&raw).map_err(err)?;
        let mut m = Mod {
            id: text(&v, "id").into(),
            version: text(&v, "version").into(),
            side: v["environment"].as_str().unwrap_or("*").into(),
            requirements: vec![],
        };
        for (key, kind) in [
            ("depends", "required"),
            ("recommends", "optional"),
            ("suggests", "suggested"),
            ("breaks", "incompatible"),
            ("conflicts", "discouraged"),
        ] {
            if let Some(map) = v[key].as_object() {
                for (id, value) in map {
                    let ranges = if let Some(s) = value.as_str() {
                        vec![s.into()]
                    } else {
                        value
                            .as_array()
                            .ok_or("Invalid Fabric dependency predicate.")?
                            .iter()
                            .map(|v| {
                                v.as_str()
                                    .map(str::to_owned)
                                    .ok_or("Invalid dependency range.".into())
                            })
                            .collect::<Result<Vec<_>>>()?
                    };
                    m.requirements.push(Requirement {
                        id: id.clone(),
                        ranges,
                        kind: kind.into(),
                        side: "BOTH".into(),
                    });
                }
            }
        }
        out.push(m);
        if let Some(jars) = v["jars"].as_array() {
            for jar in jars {
                let name = text(jar, "file");
                let nested = entry(bytes, name)?.ok_or("Declared nested Fabric JAR is missing.")?;
                out.extend(inspect_depth(&nested, target, depth + 1)?);
                if out.len() > 512 {
                    return Err("Too many bundled runtime mods.".into());
                }
            }
        }
        // Fabric aliases are real loader-provided identities, not project-name matches.
        if let Some(aliases) = v["provides"].as_array() {
            for alias in aliases {
                if let Some(id) = alias.as_str() {
                    out.push(Mod {
                        id: id.into(),
                        version: out[0].version.clone(),
                        side: out[0].side.clone(),
                        requirements: vec![],
                    });
                }
            }
        }
    } else {
        let path = if target.loader == "neoforge" {
            "META-INF/neoforge.mods.toml"
        } else {
            "META-INF/mods.toml"
        };
        let raw = entry(bytes, path)?
            .ok_or("This JAR has no runtime metadata for the selected loader.")?;
        let v: toml::Value = toml::from_str(&String::from_utf8(raw).map_err(err)?).map_err(err)?;
        let manifest = entry(bytes, "META-INF/MANIFEST.MF")?.unwrap_or_default();
        let manifest = String::from_utf8_lossy(&manifest);
        let implementation = manifest
            .lines()
            .find_map(|l| l.strip_prefix("Implementation-Version: "))
            .unwrap_or("")
            .trim();
        if let Some(mods) = v.get("mods").and_then(toml::Value::as_array) {
            for item in mods {
                let id = item
                    .get("modId")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("");
                let mut version = item
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                version = version.replace("${file.jarVersion}", implementation);
                let mut m = Mod {
                    id: id.into(),
                    version,
                    side: "*".into(),
                    requirements: vec![],
                };
                if let Some(deps) = v
                    .get("dependencies")
                    .and_then(|x| x.get(id))
                    .and_then(toml::Value::as_array)
                {
                    for d in deps {
                        let get = |key| {
                            d.get(key)
                                .and_then(toml::Value::as_str)
                                .unwrap_or("")
                                .to_string()
                        };
                        let kind = d.get("type").and_then(toml::Value::as_str).unwrap_or(
                            if d.get("mandatory")
                                .and_then(toml::Value::as_bool)
                                .unwrap_or(true)
                            {
                                "required"
                            } else {
                                "optional"
                            },
                        );
                        m.requirements.push(Requirement {
                            id: get("modId"),
                            ranges: vec![get("versionRange")],
                            kind: kind.into(),
                            side: get("side"),
                        });
                    }
                }
                if let Some(range) = v.get("loaderVersion").and_then(toml::Value::as_str) {
                    m.requirements.push(Requirement {
                        id: if target.loader == "forge" {
                            "javafml"
                        } else {
                            "neofml"
                        }
                        .into(),
                        ranges: vec![range.into()],
                        kind: "required".into(),
                        side: "BOTH".into(),
                    });
                }
                out.push(m);
            }
        }
        // Jar-in-Jar libraries are already bundled. Only inspect nested runtime mods.
        if let Some(raw) = entry(bytes, "META-INF/jarjar/metadata.json")? {
            let jarjar: Value = serde_json::from_slice(&raw).map_err(err)?;
            if let Some(jars) = jarjar["jars"].as_array() {
                for j in jars {
                    let nested =
                        entry(bytes, text(j, "path"))?.ok_or("Missing bundled library.")?;
                    if entry(&nested, path)?.is_some() {
                        out.extend(inspect_depth(&nested, target, depth + 1)?);
                        if out.len() > 512 {
                            return Err("Too many bundled runtime mods.".into());
                        }
                    }
                }
            }
        }
    }
    if out.is_empty() || out.len() > 512 {
        return Err("No supported runtime mods found, or too many bundled mods.".into());
    }
    for m in &out {
        if m.id.is_empty()
            || m.id.len() > 128
            || !m
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
            || m.version.is_empty()
            || m.version.contains("${")
        {
            return Err(
                "Mod identity/version is invalid or contains unresolved build placeholders.".into(),
            );
        }
        if !["*", "client", "server"].contains(&m.side.as_str()) {
            return Err("Unknown mod environment.".into());
        }
    }
    Ok(out)
}

fn numbers(s: &str) -> Option<Vec<u64>> {
    s.split('.').map(|x| x.parse().ok()).collect()
}
fn compare(a: &str, b: &str) -> Result<Ordering> {
    let a = a.split('+').next().unwrap();
    let b = b.split('+').next().unwrap();
    let (base_a, pre_a) = a.split_once('-').map_or((a, None), |(x, y)| (x, Some(y)));
    let (base_b, pre_b) = b.split_once('-').map_or((b, None), |(x, y)| (x, Some(y)));
    let (mut av, mut bv) = (
        numbers(base_a).ok_or("Unsupported ordered version; select a concrete compatible file.")?,
        numbers(base_b).ok_or("Unsupported ordered version constraint.")?,
    );
    let n = av.len().max(bv.len());
    av.resize(n, 0);
    bv.resize(n, 0);
    let cmp = av.cmp(&bv);
    if cmp != Ordering::Equal {
        return Ok(cmp);
    }
    Ok(match (pre_a, pre_b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => {
            let mut result = Ordering::Equal;
            for (x, y) in a.split('.').zip(b.split('.')) {
                result = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    _ => x.cmp(y),
                };
                if result != Ordering::Equal {
                    break;
                }
            }
            if result == Ordering::Equal {
                a.split('.').count().cmp(&b.split('.').count())
            } else {
                result
            }
        }
    })
}
pub fn matches(version: &str, range: &str) -> Result<bool> {
    let r = range.trim();
    if r.is_empty() || r == "*" {
        return Ok(true);
    }
    if r.starts_with(['[', '(']) {
        // Maven union of intervals, including exact [version].
        let mut rest = r;
        let mut any = false;
        while !rest.is_empty() {
            let end = rest
                .find([']', ')'])
                .ok_or("Invalid Maven version range.")?;
            let part = &rest[..=end];
            let inner = &part[1..part.len() - 1];
            let ok = if let Some((lo, hi)) = inner.split_once(',') {
                (lo.is_empty()
                    || if part.starts_with('[') {
                        compare(version, lo)? != Ordering::Less
                    } else {
                        compare(version, lo)? == Ordering::Greater
                    })
                    && (hi.is_empty()
                        || if part.ends_with(']') {
                            compare(version, hi)? != Ordering::Greater
                        } else {
                            compare(version, hi)? == Ordering::Less
                        })
            } else {
                version == inner
            };
            any |= ok;
            rest = rest[end + 1..].trim_start_matches(',').trim();
        }
        return Ok(any);
    }
    if r.contains("||") {
        for branch in r.split("||") {
            if matches(version, branch)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if r.split_whitespace().count() > 1 {
        for term in r.split_whitespace() {
            if !matches(version, term)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    for op in [">=", "<=", ">", "<", "=", "~", "^"] {
        if let Some(v) = r.strip_prefix(op) {
            if op == "=" {
                return Ok(version == v || compare(version, v).is_ok_and(|c| c == Ordering::Equal));
            }
            let cmp = compare(version, v)?;
            return Ok(match op {
                ">=" => cmp != Ordering::Less,
                "<=" => cmp != Ordering::Greater,
                ">" => cmp == Ordering::Greater,
                "<" => cmp == Ordering::Less,
                _ => {
                    let mut upper = numbers(v).ok_or("Unsupported version range.")?;
                    let index = if op == "~" {
                        if upper.len() > 1 {
                            1
                        } else {
                            0
                        }
                    } else {
                        upper
                            .iter()
                            .position(|n| *n != 0)
                            .unwrap_or(upper.len() - 1)
                    };
                    upper[index] += 1;
                    for n in upper.iter_mut().skip(index + 1) {
                        *n = 0;
                    }
                    cmp != Ordering::Less
                        && compare(
                            version,
                            &upper
                                .iter()
                                .map(u64::to_string)
                                .collect::<Vec<_>>()
                                .join("."),
                        )? == Ordering::Less
                }
            });
        }
    }
    if r.split('.').any(|x| ["*", "x", "X"].contains(&x)) {
        let mut actual = version.split('.');
        for part in r.split('.') {
            if ["*", "x", "X"].contains(&part) {
                return Ok(true);
            }
            if actual.next() != Some(part) {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    Ok(version == r || compare(version, r).is_ok_and(|c| c == Ordering::Equal))
}
pub fn satisfies(version: &str, r: &Requirement) -> Result<bool> {
    for range in &r.ranges {
        if matches(version, range)? {
            return Ok(true);
        }
    }
    Ok(false)
}
pub fn builtin(id: &str, t: Target) -> Option<&str> {
    match id {
        "minecraft" => Some(t.minecraft_version),
        "java" => Some("21"),
        "fabricloader" if t.loader == "fabric" => Some(t.loader_version),
        "forge" if t.loader == "forge" => Some(t.loader_version),
        "neoforge" if t.loader == "neoforge" => Some(t.loader_version),
        "javafml" if t.loader == "forge" => Some("52"),
        "neofml" if t.loader == "neoforge" => Some("4"),
        _ => None,
    }
}
