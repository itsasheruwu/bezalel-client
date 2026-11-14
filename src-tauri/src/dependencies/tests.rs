use super::*;
use serde_json::json;
use sha2::{Digest, Sha512};
use std::{
    future::Future,
    io::{Cursor, Write},
};

fn jar(v: serde_json::Value) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    z.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    z.write_all(v.to_string().as_bytes()).unwrap();
    z.finish().unwrap().into_inner()
}
fn mod_jar(id: &str, deps: serde_json::Value) -> Vec<u8> {
    jar(json!({"schemaVersion":1,"id":id,"version":"1.0.0","depends":deps}))
}
fn mods(id: &str, deps: serde_json::Value) -> Vec<Mod> {
    metadata::inspect(&mod_jar(id, deps), state::TARGETS[0]).unwrap()
}
struct Fixture {
    state: State,
    pack: Uuid,
}
impl Fixture {
    fn new() -> Self {
        let state = State::new(
            std::env::temp_dir().join(format!("bezalel-dependency-test-{}", Uuid::new_v4())),
        )
        .unwrap();
        let pack = Uuid::new_v4();
        state
            .update(|d| {
                d.settings.dependency_offline = true;
                d.packs.push(state::Pack {
                    id: pack,
                    name: "Dependency test pack".into(),
                    target_id: "fabric-1.21.1".into(),
                    address: String::new(),
                    owner_id: None,
                    role: "owner".into(),
                    trusted_owner_id: None,
                    active_revision: None,
                    installed_revision: None,
                });
                Ok(())
            })
            .unwrap();
        Self { state, pack }
    }
    fn dir(&self) -> PathBuf {
        self.state.game_dir(&self.state.data().unwrap(), self.pack)
    }
    fn request(&self, action: &str) -> permissions::Request {
        permissions::new_request(
            Scope {
                pack_id: self.pack,
                target_id: "fabric-1.21.1".into(),
                action: action.into(),
                sources: vec!["Modrinth".into()],
            },
            None,
            "test",
            "Dependency test pack",
            "Test plan",
            vec![],
            vec![],
            vec![],
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        /* SQLite remains open until State drops; remove JAR fixtures without modifying any real pack. */
        let _ = fs::remove_dir_all(self.dir());
    }
}
fn decision(r: &permissions::Request, s: &str) -> permissions::Decision {
    permissions::Decision {
        decision: s.into(),
        fingerprint: r.fingerprint.clone(),
        choice: None,
        file: None,
    }
}
async fn answer<T>(
    state: &State,
    operation: impl Future<Output = Result<T>>,
    decide: impl Fn(&permissions::Request) -> permissions::Decision,
) -> Result<T> {
    tokio::pin!(operation);
    loop {
        tokio::select! {result=&mut operation=>return result,_=tokio::time::sleep(std::time::Duration::from_millis(5))=>{for r in permissions::list(state)?{permissions::decide(state,r.id,decide(&r))?;}}}
    }
}
#[test]
fn fabric_ranges_and_maven_ranges() {
    for (v, r, expected) in [
        ("1.21.1", ">=1.21 <1.22", true),
        ("1.21.1", "~1.21.0", true),
        ("0.2.4", "^0.2.3", true),
        ("0.3.0", "^0.2.3", false),
        ("6.1.0", "[3.0.0,7)", true),
        ("1.5", "(,1.0],[1.2,)", true),
        ("1.1", "(,1.0],[1.2,)", false),
        ("1.21.1", "1.21.x", true),
        ("1.21.1", "1.20.x || 1.21.x", true),
        ("1.0.0", "1.0.0-beta.1", false),
        ("1.0.0-beta.2", ">1.0.0-beta.1", true),
        ("1.21.1", ">=1.21-", true),
        ("release", "release", true),
    ] {
        assert_eq!(metadata::matches(v, r).unwrap(), expected, "{v} {r}");
    }
    assert!(metadata::matches("custom", ">=alpha").is_err());
}
#[test]
fn graphs_missing_recursive_cycles_optional_and_conflicts() {
    let t = state::TARGETS[0];
    let a = mods("a", json!({"b":">=1"}));
    let b = mods("b", json!({"c":"*"}));
    let c = mods("c", json!({"a":"*"}));
    let mut all = BTreeMap::new();
    for m in a.iter().chain(&b) {
        all.insert(m.id.clone(), m.clone());
    }
    assert_eq!(validate(&all, t, "client").unwrap()[0].1.id, "c");
    all.insert("c".into(), c[0].clone());
    assert!(validate(&all, t, "client").unwrap().is_empty());
    let optional=metadata::inspect(&jar(json!({"id":"optional","version":"1","suggests":{"absent":"*"},"recommends":{"b":">=2"}})),t).unwrap();
    all.insert("optional".into(), optional[0].clone());
    assert!(validate(&all, t, "client").unwrap_err().contains("needs b"));
    all.remove("optional");
    let bad = metadata::inspect(
        &jar(json!({"id":"bad","version":"1","breaks":{"a":"*"}})),
        t,
    )
    .unwrap();
    all.insert("bad".into(), bad[0].clone());
    assert!(validate(&all, t, "client")
        .unwrap_err()
        .contains("conflicts"));
}
#[test]
fn compatibility_loader_side_build_libraries_and_duplicate_identity() {
    let bytes = mod_jar("a", json!({"minecraft":"1.20.x"}));
    assert!(validate(
        &index(&[], &metadata::inspect(&bytes, state::TARGETS[0]).unwrap()).unwrap(),
        state::TARGETS[0],
        "client"
    )
    .is_err());
    assert!(metadata::inspect(&bytes, state::TARGETS[1]).is_err());
    let m = metadata::inspect(
        &jar(json!({"id":"server_mod","version":"1","environment":"server"})),
        state::TARGETS[0],
    )
    .unwrap();
    assert!(validate(&index(&[], &m).unwrap(), state::TARGETS[0], "client").is_err());
    assert!(index(&[], &[mods("a", json!({})), mods("a", json!({}))].concat()).is_err());
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    z.start_file("library.class", zip::write::SimpleFileOptions::default())
        .unwrap();
    z.write_all(b"build-only library").unwrap();
    assert!(metadata::inspect(&z.finish().unwrap().into_inner(), state::TARGETS[0]).is_err());
}
#[test]
fn forge_and_neoforge_runtime_metadata_and_sided_dependencies() {
    for (target, path, kind) in [
        (state::TARGETS[1], "META-INF/mods.toml", "mandatory = true"),
        (
            state::TARGETS[2],
            "META-INF/neoforge.mods.toml",
            "type = 'required'",
        ),
    ] {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        z.start_file(path, zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(format!("modLoader='javafml'\nloaderVersion='[{},)'\n[[mods]]\nmodId='example'\nversion='1.2'\n[[dependencies.example]]\nmodId='server_only'\n{kind}\nversionRange='[1,)'\nside='SERVER'\n",if target.loader=="forge"{"52"}else{"4"}).as_bytes()).unwrap();
        let m = metadata::inspect(&z.finish().unwrap().into_inner(), target).unwrap();
        assert!(validate(&index(&[], &m).unwrap(), target, "client")
            .unwrap()
            .is_empty());
        assert_eq!(
            validate(&index(&[], &m).unwrap(), target, "server").unwrap()[0]
                .1
                .id,
            "server_only"
        );
    }
}
#[tokio::test]
async fn permissions_pause_allow_deny_replay_fingerprint_and_isolation() {
    let f = Fixture::new();
    let r = f.request("Install");
    let id = r.id;
    let fp = r.fingerprint.clone();
    let result = answer(&f.state, permissions::ask(&f.state, r.clone()), |r| {
        let mut stale = decision(r, "allow");
        stale.fingerprint = "changed".into();
        assert!(permissions::decide(&f.state, r.id, stale).is_err());
        decision(r, "allow")
    })
    .await
    .unwrap();
    assert_eq!(result.decision, "allow");
    assert!(permissions::decide(
        &f.state,
        id,
        permissions::Decision {
            decision: "allow".into(),
            fingerprint: fp,
            choice: None,
            file: None
        }
    )
    .is_err());
    assert!(answer(
        &f.state,
        permissions::ask(&f.state, f.request("Search and stage")),
        |r| decision(r, "deny")
    )
    .await
    .unwrap_err()
    .contains("denied"));
    let other = Fixture::new();
    let r = f.request("Install");
    answer(&f.state, permissions::ask(&f.state, r), |r| {
        assert!(permissions::decide(&other.state, r.id, decision(r, "allow")).is_err());
        decision(r, "allow")
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn always_allow_persists_scopes_revocation_and_replacements_ask() {
    let f = Fixture::new();
    let r = f.request("Install");
    answer(&f.state, permissions::ask(&f.state, r.clone()), |r| {
        decision(r, "always")
    })
    .await
    .unwrap();
    let reopened = State::new(f.state.root.clone()).unwrap();
    assert_eq!(reopened.data().unwrap().dependency_grants.len(), 1);
    permissions::ask(&f.state, f.request("Install"))
        .await
        .unwrap();
    assert!(permissions::list(&f.state).unwrap().is_empty());
    let mut changed = r.clone();
    changed.scope.pack_id = Uuid::new_v4();
    changed.id = Uuid::new_v4();
    assert!(
        answer(&f.state, permissions::ask(&f.state, changed), |r| decision(
            r, "deny"
        ))
        .await
        .is_err()
    );
    let mut replacement = r;
    replacement.id = Uuid::new_v4();
    replacement.changes.push("Replace old.jar".into());
    assert!(
        answer(&f.state, permissions::ask(&f.state, replacement), |r| {
            assert!(permissions::decide(&f.state, r.id, decision(r, "always")).is_err());
            decision(r, "deny")
        })
        .await
        .is_err()
    );
    f.state
        .update(|d| {
            d.dependency_grants.clear();
            Ok(())
        })
        .unwrap();
    assert!(answer(
        &f.state,
        permissions::ask(&f.state, f.request("Install")),
        |r| decision(r, "deny")
    )
    .await
    .is_err());
}
#[tokio::test]
async fn cancellation_drops_pending_and_denied_import_has_no_effect() {
    let f = Fixture::new();
    let r = f.request("Install");
    let id = r.id;
    let mut wait = Box::pin(permissions::ask(&f.state, r));
    tokio::select! {_=&mut wait=>panic!("must wait"),_=tokio::time::sleep(std::time::Duration::from_millis(10))=>{}}
    assert_eq!(permissions::list(&f.state).unwrap().len(), 1);
    drop(wait);
    assert!(permissions::list(&f.state).unwrap().is_empty());
    let input = f.state.root.join("root.jar");
    fs::write(&input, mod_jar("root", json!({}))).unwrap();
    assert!(
        answer(&f.state, import(&f.state, f.pack, None, &input), |r| {
            decision(r, "deny")
        })
        .await
        .is_err()
    );
    assert!(!f.dir().join("mods/root.jar").exists());
    assert!(permissions::decide(&f.state, id, decision(&f.request("Install"), "allow")).is_err());
}
#[tokio::test]
async fn offline_missing_and_changed_plan_are_rejected() {
    let f = Fixture::new();
    let missing = prepare(
        &f.state,
        f.pack,
        None,
        "root.jar",
        mod_jar("root", json!({"missing":"*"})),
        None,
    )
    .await;
    assert!(missing.err().unwrap().contains("Offline"));
    let operation = prepare(
        &f.state,
        f.pack,
        None,
        "root.jar",
        mod_jar("root", json!({})),
        None,
    );
    let result = answer(&f.state, operation, |r| {
        fs::create_dir_all(f.dir().join("mods")).unwrap();
        fs::write(
            f.dir().join("mods/external.jar"),
            mod_jar("external", json!({})),
        )
        .unwrap();
        decision(r, "allow")
    })
    .await;
    assert!(result.err().unwrap().contains("changed"));
    assert!(!f.dir().join("mods/root.jar").exists());
}
#[test]
fn checksum_failures_restricted_urls_and_rollback() {
    let f = Fixture::new();
    let bytes = mod_jar("root", json!({}));
    let r = Remote {
        source: "Modrinth".into(),
        project: "project".into(),
        version_id: "version".into(),
        version: "1".into(),
        filename: "root.jar".into(),
        url: "https://cdn.modrinth.com/data/a.jar".into(),
        sha512: Some(format!("{:x}", Sha512::digest(&bytes))),
        sha1: None,
        size: bytes.len() as u64,
        deps: vec![],
        client: true,
        server: true,
    };
    assert!(providers::verify(&r, &bytes).is_ok());
    assert!(providers::verify(&r, b"partial").is_err());
    let mut bad = bytes.clone();
    bad[0] ^= 1;
    assert!(providers::verify(&r, &bad).is_err());
    assert!(providers::validate_url("https://evil.example/mod.jar", "Modrinth").is_err());
    assert!(providers::validate_url("http://cdn.modrinth.com/mod.jar", "Modrinth").is_err());
    let a = Artifact {
        filename: "root.jar".into(),
        mods: mods("root", json!({})),
        bytes,
        remote: None,
        reason: "test".into(),
    };
    let mut b = a.clone();
    b.filename = "locked.jar".into();
    fs::create_dir_all(f.dir().join("mods/locked.jar")).unwrap();
    let p = Prepared {
        inventory: inventory(&f.dir()).unwrap_or_default(),
        artifacts: vec![a, b],
        records: vec![],
    }; // inventory rejects this collision before commit too
    assert!(commit_import(&f.state, f.pack, &f.dir(), p).is_err());
    assert!(!f.dir().join("mods/root.jar").exists());
    // A late file collision rolls back earlier staged creations.
    {
        let mut changes = crate::mod_installation::JarChanges::default();
        changes
            .create(&f.dir().join("mods/first.jar"), b"first")
            .unwrap();
        assert!(changes
            .create(&f.dir().join("mods/locked.jar"), b"second")
            .is_err());
    }
    assert!(!f.dir().join("mods/first.jar").exists());
}

#[tokio::test]
#[ignore = "Uses real Modrinth API/CDN; run explicitly with network access"]
async fn live_import_resolves_trinkets_then_fabric_api_and_installs_approved_files() {
    let f = Fixture::new();
    f.state
        .update(|d| {
            d.settings.dependency_offline = false;
            Ok(())
        })
        .unwrap();
    let input = f.state.root.join("imported-example.jar");
    fs::write(
        &input,
        mod_jar("imported_example", json!({"trinkets":">=3.10.0"})),
    )
    .unwrap();
    let mut seen = HashSet::new();
    let operation = import(&f.state, f.pack, None, &input);
    let result = answer(&f.state, operation, |r| {
        println!("APPROVAL {}: {}", r.scope.action, r.fingerprint);
        assert_ne!(
            r.scope.action, "Choose project",
            "Verified dependencies should resolve automatically"
        );
        let mut d = decision(r, "allow");
        if !r.choices.is_empty() {
            d.choice = r
                .choices
                .iter()
                .find(|s| s.starts_with("Modrinth:5aaWibi9 "))
                .cloned();
            assert!(
                d.choice.is_some(),
                "Trinkets identity missing in live search"
            );
        }
        d
    })
    .await;
    result.unwrap();
    let files = inventory(&f.dir()).unwrap();
    let mut all = vec![];
    for (name, hash) in &files {
        let bytes = fs::read(f.dir().join("mods").join(name)).unwrap();
        let mods = metadata::inspect(&bytes, state::TARGETS[0]).unwrap();
        for m in mods {
            seen.insert(m.id.clone());
            all.push(m);
        }
        println!("INSTALLED {name} sha256={hash}");
    }
    assert!(
        seen.contains("imported_example")
            && seen.contains("trinkets")
            && seen.contains("fabric-api")
    );
    assert_eq!(files.len(), 3);
    assert!(
        validate(&index(&[], &all).unwrap(), state::TARGETS[0], "client")
            .unwrap()
            .is_empty()
    );
    let records = f.state.data().unwrap().dependency_records;
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|r| r.pack_id == f.pack));
    assert!(records
        .iter()
        .any(|r| r.required_by.contains(&"trinkets".into())));
}

#[tokio::test]
async fn simultaneous_requests_and_changed_source_scopes_are_independent() {
    let f = Fixture::new();
    let first = f.request("Install");
    let second = f.request("Search and stage");
    let both = async {
        let (a, b) = tokio::join!(
            permissions::ask(&f.state, first),
            permissions::ask(&f.state, second)
        );
        Ok((a, b))
    };
    let (a, b) = answer(&f.state, both, |r| {
        decision(
            r,
            if r.scope.action == "Install" {
                "allow"
            } else {
                "deny"
            },
        )
    })
    .await
    .unwrap();
    assert!(a.is_ok());
    assert!(b.is_err());
    let request = f.request("Install");
    let changed = permissions::new_request(
        Scope {
            sources: vec!["CurseForge".into()],
            ..request.scope.clone()
        },
        None,
        "test",
        "Dependency test pack",
        "Test plan",
        vec![],
        vec![],
        vec![],
    );
    assert_ne!(request.fingerprint, changed.fingerprint);
}
#[test]
fn missing_optional_mods_are_skipped_and_conflicting_intersections_are_rejected() {
    let bytes = jar(
        json!({"id":"example","version":"1","recommends":{"absent":">=2"},"suggests":{"other":"*"},"depends":{"fabricloader":">=0.16","minecraft":"~1.21.1","java":">=21"}}),
    );
    let m = metadata::inspect(&bytes, state::TARGETS[0]).unwrap();
    assert!(
        validate(&index(&[], &m).unwrap(), state::TARGETS[0], "client")
            .unwrap()
            .is_empty()
    );
    let mut m = mods("a", json!({"common":">=2"}));
    m.extend(mods("b", json!({"common":"<2"})));
    m.extend(mods("common", json!({})));
    assert!(validate(&index(&[], &m).unwrap(), state::TARGETS[0], "client").is_err());
}
#[tokio::test]
async fn download_http_errors_partial_bodies_and_rate_limits_never_mutate_pack() {
    use tokio::io::AsyncWriteExt;
    let f = Fixture::new();
    let bytes = mod_jar("mod", json!({}));
    let remote = Remote {
        source: "Modrinth".into(),
        project: "test".into(),
        version_id: "release".into(),
        version: "1".into(),
        filename: "mod.jar".into(),
        url: "https://cdn.modrinth.com/data/test.jar".into(),
        sha512: Some(format!("{:x}", Sha512::digest(&bytes))),
        sha1: None,
        size: bytes.len() as u64,
        deps: vec![],
        client: true,
        server: true,
    };
    for (status, body) in [
        ("500 Internal Server Error", b"failed".as_slice()),
        ("429 Too Many Requests", b"rate".as_slice()),
        ("200 OK", b"partial".as_slice()),
        ("302 Found", b"".as_slice()),
    ] {
        // Exercise the production HTTP response reader against a real socket. Production downloads still enforce the provider CDN allowlist.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response=format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nRetry-After: 17\r\nConnection: close\r\n\r\n{}",body.len(),String::from_utf8_lossy(body));
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            use tokio::io::AsyncReadExt;
            let mut input = [0; 2048];
            let _ = stream.read(&mut input).await;
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let response = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
            .get(format!("http://{address}/file"))
            .send()
            .await
            .unwrap();
        let result = providers::read_download(response, &remote).await;
        assert!(result.is_err());
        if status.starts_with("429") {
            assert!(result.unwrap_err().contains("17"));
        }
        server.await.unwrap();
        assert!(inventory(&f.dir()).unwrap().is_empty());
    }
}
#[test]
fn bundled_fabric_dependencies_are_inspected_without_installing_build_libraries() {
    let nested = mod_jar("bundled", json!({"minecraft":"1.21.x"}));
    let outer = json!({"id":"outer","version":"1","depends":{"bundled":"*"},"jars":[{"file":"META-INF/jars/bundled.jar"}]});
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in [
        ("fabric.mod.json", outer.to_string().into_bytes()),
        ("META-INF/jars/bundled.jar", nested),
    ] {
        z.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(&bytes).unwrap();
    }
    let m = metadata::inspect(&z.finish().unwrap().into_inner(), state::TARGETS[0]).unwrap();
    assert_eq!(m.len(), 2);
    assert!(
        validate(&index(&[], &m).unwrap(), state::TARGETS[0], "client")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn automatic_matching_prefers_real_mod_over_compatibility_alias_and_refuses_ambiguity() {
    let direct = Artifact {
        filename: "original.jar".into(),
        bytes: vec![],
        mods: mods("wanted", json!({})),
        remote: None,
        reason: "test".into(),
    };
    let mut adapter = direct.clone();
    adapter.filename = "adapter.jar".into();
    adapter.mods = mods("adapter", json!({}));
    adapter.mods.extend(mods("wanted", json!({})));
    assert_eq!(
        unique_match(vec![adapter, direct.clone()], "wanted")
            .unwrap()
            .filename,
        "original.jar"
    );
    assert!(unique_match(vec![direct.clone(), direct], "wanted").is_err());
    assert!(unique_match(vec![], "wanted").is_err());
}
