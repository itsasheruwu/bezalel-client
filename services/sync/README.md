# Bezalel private pack service

This service keeps profiles, private pack membership, and release metadata in PostgreSQL. It uses a private S3-compatible bucket for submitted JARs and managed config. The hosted beta uses private Bezalel access codes for profiles. Minecraft game sign-in and ChatGPT sign-in are handled separately by the launcher.

## Run the service

1. Create a PostgreSQL database for Bezalel and a private S3 bucket. Give the service access to get and put objects under the bucket's `packs/` prefix. The service creates its tables on first startup.
2. Run `cargo run --manifest-path services/sync/Cargo.toml -- --generate-key`. Store the private seed with your service secrets and give the public key to testers with the service URL. Preserve the key across restarts and deploys so older releases remain verifiable.
3. Set the variables below and start the service with `cargo run --manifest-path services/sync/Cargo.toml --release`.
4. Keep the local API on loopback for local tests. The launcher accepts plain HTTP only on loopback.

| Variable | Value |
|---|---|
| `DATABASE_URL` | PostgreSQL connection string. Use verified TLS for a remote database. |
| `BEZALEL_SYNC_LISTEN` | Optional explicit HTTP listen address. Without it, `PORT` binds the server on `0.0.0.0`. If neither is set, the service uses `127.0.0.1:3748`. |
| `BEZALEL_SYNC_SIGNING_KEY` | Private 32-byte seed in hexadecimal. |
| `BEZALEL_ADMIN_TOKEN` | Local test account provisioning secret with 32–256 printable ASCII characters. Leave unset for the hosted beta. |
| `BEZALEL_OWNER_ACCESS_CODE_SHA256` | SHA-256 hex of the beta owner's private access code. Generate the code from 16 random bytes, encode as 32 lowercase hex characters, and hash that normalized string. |
| `BEZALEL_S3_BUCKET` | Name of the private bucket. |
| `AWS_REGION` | Region expected by your bucket provider. |
| `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` | Service credentials for the private bucket. |
| `AWS_ENDPOINT` | Set the endpoint for an S3-compatible provider. Include the bucket in the hostname when using virtual-hosted style. Leave unset for AWS S3. |

For a local smoke test, set `BEZALEL_ADMIN_TOKEN` and use `POST /v1/admin/accounts` on the loopback service. Send `Authorization: Bearer <operator-token>` and JSON such as `{"name":"Alex"}`. Use the returned tester token with the launcher's local connection form. The service stores a hash of each token.

## Exercise the release flow

1. Connect the owner and friend accounts separately. As owner, create a pack, make a player invite, and confirm the friend can preview it and accept trust for that owner. Before the friend joins, confirm their account cannot list or download the pack.
2. Build a JAR for the pack's target. Submit it and its source changes as owner or editor. Confirm a player cannot see the pending artifact. A submission accepts up to 16 MiB for a JAR and 1 MiB of text changes. The launcher supplies local test results and gates submission. The service checks the JAR format and target loader metadata and relies on the owner to review the proposal and test the server.
3. Prepare two candidates from the same active revision. Export the first server bundle, back up the world, apply it to a compatible server, restart, and test a join. Verify publishing with the confirmation checkbox clear fails. Confirm and publish the first candidate and verify publishing the second returns a conflict. Repeating the first publication returns the same immutable release.
4. Open the friend's launcher and verify the published pack downloads and starts. Interrupt a later download and verify the existing pack and saves remain available. Publish an older release as a new rollback candidate only after applying it and any needed world backup on the server.
5. Revoke the invite and verify a third account cannot join with its code. Remove the friend and verify that account loses service access to the pack. Separately, test that revoking trust in the launcher stops running owner updates on that PC.

Fabric 1.21.1 packs automatically include Fabric API 0.110.0+1.21.1. The launcher fetches this pinned dependency directly from Fabric Maven for both installs and server bundles. The service stores submitted JARs and managed text config under `config/bezalel/`. Minecraft client and server files come from the game or loader's distribution channels.

The service signs candidates for server export and uses a separate signature context for published releases. Candidate signatures cannot activate a client install. PostgreSQL serializes publication of each pack and atomically advances its active revision. Back up the database, bucket, and signing key together. Release history continues to reference older artifacts for rollback.
