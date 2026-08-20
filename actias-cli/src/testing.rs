//! `actias test`: runs a project's `tests/*.lua` on the same runtime the
//! platform uses, with the kv service faked in memory behind the identical
//! grpc surface and secrets encrypted into it exactly as production stores
//! them. What passes here runs the same way on a worker.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use actias_worker_core::proto::kv_service::kv_service_client::KvServiceClient;
use actias_worker_core::proto::script_service::{Revision, Script};
use actias_worker_core::runtime::{ActiasRuntime, PreparedRevision};
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit};
use base64::Engine;
use colored::*;

use crate::script::ScriptConfig;

/// Project id every fake pair and runtime share.
const TEST_PROJECT: &str = "test-project";

mod proto {
    tonic::include_proto!("kv_service");
}

/// Outcome of one `actias test` run.
pub struct TestSummary {
    pub passed: usize,
    pub failed: usize,
}

/// A pair's full address: project, namespace, key.
type PairKey = (String, String, String);

/// The kv service over a hash map: the same wire surface, none of the
/// storage. One store lives exactly as long as one test file.
#[derive(Default, Clone)]
struct FakeKv {
    pairs: Arc<Mutex<HashMap<PairKey, proto::Pair>>>,
}

impl FakeKv {
    fn insert(&self, pair: proto::Pair) {
        self.pairs.lock().expect("no other holder").insert(
            (
                pair.project_id.clone(),
                pair.namespace.clone(),
                pair.key.clone(),
            ),
            pair,
        );
    }
}

#[tonic::async_trait]
impl proto::kv_service_server::KvService for FakeKv {
    async fn get_pair(
        &self,
        request: tonic::Request<proto::PairRequest>,
    ) -> Result<tonic::Response<proto::Pair>, tonic::Status> {
        let request = request.into_inner();
        let key = (request.project_id, request.namespace, request.key);

        match self.pairs.lock().expect("no other holder").get(&key) {
            Some(pair) => Ok(tonic::Response::new(pair.clone())),
            None => Err(tonic::Status::not_found("No pair with that key.")),
        }
    }

    async fn set_pairs(
        &self,
        request: tonic::Request<proto::SetPairsRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        for pair in request.into_inner().pairs {
            self.insert(pair);
        }
        Ok(tonic::Response::new(()))
    }

    async fn list_pairs(
        &self,
        request: tonic::Request<proto::ListPairsRequest>,
    ) -> Result<tonic::Response<proto::ListPairsResponse>, tonic::Status> {
        let request = request.into_inner();
        let pairs: Vec<proto::Pair> = self
            .pairs
            .lock()
            .expect("no other holder")
            .values()
            .filter(|pair| {
                pair.project_id == request.project_id && pair.namespace == request.namespace
            })
            .cloned()
            .collect();

        Ok(tonic::Response::new(proto::ListPairsResponse {
            page_size: pairs.len() as i32,
            token: None,
            pairs,
        }))
    }

    async fn delete_pairs(
        &self,
        request: tonic::Request<proto::DeletePairsRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let mut pairs = self.pairs.lock().expect("no other holder");
        for target in request.into_inner().pairs {
            pairs.remove(&(target.project_id, target.namespace, target.key));
        }
        Ok(tonic::Response::new(()))
    }

    async fn create_namespace(
        &self,
        request: tonic::Request<proto::CreateNamespaceRequest>,
    ) -> Result<tonic::Response<proto::Namespace>, tonic::Status> {
        let request = request.into_inner();
        Ok(tonic::Response::new(proto::Namespace {
            project_id: request.project_id,
            name: request.namespace,
            count: 0,
        }))
    }

    async fn list_namespaces(
        &self,
        request: tonic::Request<proto::ListNamespacesRequest>,
    ) -> Result<tonic::Response<proto::ListNamespacesResponse>, tonic::Status> {
        let request = request.into_inner();
        let mut names: Vec<String> = self
            .pairs
            .lock()
            .expect("no other holder")
            .values()
            .filter(|pair| pair.project_id == request.project_id)
            .map(|pair| pair.namespace.clone())
            .collect();
        names.sort();
        names.dedup();

        Ok(tonic::Response::new(proto::ListNamespacesResponse {
            namespaces: names
                .into_iter()
                .map(|name| proto::Namespace {
                    project_id: request.project_id.clone(),
                    name,
                    count: 0,
                })
                .collect(),
        }))
    }

    async fn delete_project(
        &self,
        request: tonic::Request<proto::DeleteProjectRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let project = request.into_inner().project_id;
        self.pairs
            .lock()
            .expect("no other holder")
            .retain(|(p, _, _), _| p != &project);
        Ok(tonic::Response::new(()))
    }

    async fn delete_namespace(
        &self,
        request: tonic::Request<proto::DeleteNamespaceRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        let request = request.into_inner();
        self.pairs
            .lock()
            .expect("no other holder")
            .retain(|(p, n, _), _| p != &request.project_id || n != &request.namespace);
        Ok(tonic::Response::new(()))
    }
}

/// 32 random bytes without pulling a rng dependency: uuids are os-random.
fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = Vec::new();
    while bytes.len() < N {
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    bytes[..N].try_into().expect("sliced to size")
}

/// Encrypts one secret value the way the api stores it, so the runtime's
/// real decryption path runs in tests too.
fn encrypt_secret(key: &[u8; 32], value: &str) -> Result<String, String> {
    let nonce: [u8; 12] = random_bytes();
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let ciphertext = cipher
        .encrypt(&nonce.into(), value.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut data = nonce.to_vec();
    data.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::STANDARD.encode(data))
}

/// Serves one fake store on a loopback port and hands back a connected
/// client; the server task dies with the process.
async fn serve_fake_kv(
    store: FakeKv,
) -> Result<KvServiceClient<tonic::transport::Channel>, String> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .map_err(|e| e.to_string())?;
    let address = listener.local_addr().map_err(|e| e.to_string())?;

    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(proto::kv_service_server::KvServiceServer::new(store))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );

    KvServiceClient::connect(format!("http://{address}"))
        .await
        .map_err(|e| e.to_string())
}

/// Builds the prepared revision the runtime executes: the project's bundle
/// with its capability contract derived from the code, exactly as publish
/// stores it.
fn prepare(config: &ScriptConfig) -> Result<Arc<PreparedRevision>, String> {
    let bundle = config.to_bundle()?;

    let mut files = Vec::with_capacity(bundle.files.len());
    for file in &bundle.files {
        let content = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(&file.content)
            .map_err(|e| format!("{}: {e}", file.file_path))?;

        files.push(actias_worker_core::proto::bundle::File {
            file_path: file.file_path.clone(),
            content,
            ..Default::default()
        });
    }

    let declared = crate::capabilities::extract(config)?;

    let script = Script {
        id: TEST_PROJECT.to_owned(),
        project_id: TEST_PROJECT.to_owned(),
        public_identifier: "test".to_owned(),
        ..Default::default()
    };

    let revision = Revision {
        bundle: Some(actias_worker_core::proto::bundle::Bundle {
            entry_point: config.entry_point.clone(),
            files,
        }),
        script_config: Some(actias_worker_core::proto::script_service::ScriptConfig {
            id: TEST_PROJECT.to_owned(),
            entry_point: config.entry_point.clone(),
            includes: vec![],
            ignore: vec![],
            capabilities: Some(actias_worker_core::proto::script_service::Capabilities {
                kv: declared.kv,
                events: declared.events,
                secrets: declared.secrets,
                objects: declared.objects,
                databases: declared.databases,
                queues: declared.queues,
                workflows: declared.workflows,
            }),
        }),
        ..Default::default()
    };

    PreparedRevision::prepare(script, revision)
        .map_err(|e| e.to_string())
        .map(Arc::new)
}

/// Test files, sorted for a stable run order.
fn test_files(config: &ScriptConfig) -> Result<Vec<PathBuf>, String> {
    let root = config
        .project_path
        .as_ref()
        .ok_or("Project has no path.")?
        .join("tests");

    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("lua") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Secret values for tests, from `tests/secrets.json` when present.
fn test_secrets(config: &ScriptConfig) -> Result<HashMap<String, String>, String> {
    let Some(root) = config.project_path.as_ref() else {
        return Ok(HashMap::new());
    };

    match std::fs::read_to_string(root.join("tests/secrets.json")) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| format!("tests/secrets.json: {e}")),
        Err(_) => Ok(HashMap::new()),
    }
}

/// Runs every test file and prints per-case results.
pub async fn run_tests(config: &ScriptConfig) -> Result<TestSummary, String> {
    let files = test_files(config)?;
    if files.is_empty() {
        return Err("No tests found; add tests/*.lua to the project.".to_owned());
    }

    let prepared = prepare(config)?;
    let secrets = test_secrets(config)?;
    let key: [u8; 32] = random_bytes();

    let mut summary = TestSummary {
        passed: 0,
        failed: 0,
    };

    for file in files {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("test")
            .to_owned();
        println!("🧪 {}", name.purple());

        // Each file gets its own store and vm, so no state leaks between
        // files; cases within one file share both on purpose.
        let store = FakeKv::default();
        for (secret_name, value) in &secrets {
            store.insert(proto::Pair {
                project_id: TEST_PROJECT.to_owned(),
                namespace: actias_common::naming::SECRETS_NAMESPACE.to_owned(),
                r#type: proto::ValueType::String as i32,
                ttl: None,
                key: secret_name.clone(),
                value: encrypt_secret(&key, value)?,
            });
        }

        let client = serve_fake_kv(store).await?;
        let egress = actias_worker_core::egress::EgressClient::new(
            actias_worker_core::egress::EgressPolicy::new([], false),
        )
        .map_err(|e| e.to_string())?;

        let runtime = ActiasRuntime::new(
            prepared.clone(),
            client,
            egress,
            None,
            Some(Arc::new(key)),
            None,
        )
        .await
        .map_err(|e| format!("{name}: the entry point failed: {e}"))?;

        // The registry the `test` global fills; the file runs and only
        // registers, then each case runs by itself so one failure never
        // hides another.
        runtime
            .load(
                r#"
                __tests = {}
                function test(name, fn)
                    table.insert(__tests, { name = name, fn = fn })
                end
                "#,
            )
            .exec()
            .map_err(|e| e.to_string())?;

        // The handler under test, dispatched exactly as a request would be.
        if let Ok(listener) = runtime.listener(ActiasRuntime::FETCH_EVENT) {
            runtime
                .globals()
                .set("fetch", listener)
                .map_err(|e| e.to_string())?;
        }

        let source =
            std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;

        runtime.start_timer();
        if let Err(error) = runtime.load(&source).set_name(&name).exec_async().await {
            println!("  {} the file itself failed: {error}", "❌".red());
            summary.failed += 1;
            continue;
        }

        let cases: mlua::Table = runtime
            .globals()
            .get("__tests")
            .map_err(|e| e.to_string())?;
        for entry in cases.sequence_values::<mlua::Table>() {
            let entry = entry.map_err(|e| e.to_string())?;
            let case_name: String = entry.get("name").unwrap_or_else(|_| "unnamed".to_owned());
            let function: mlua::Function = entry.get("fn").map_err(|e| e.to_string())?;

            match function.call_async::<()>(()).await {
                Ok(()) => {
                    println!("  {} {case_name}", "✅".green());
                    summary.passed += 1;
                }
                Err(error) => {
                    println!("  {} {case_name}: {error}", "❌".red());
                    summary.failed += 1;
                }
            }
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A project on disk with the given main.lua and one test file.
    fn project(dir: &std::path::Path, main: &str, test: &str) -> ScriptConfig {
        let mut file = std::fs::File::create(dir.join("main.lua")).expect("main");
        file.write_all(main.as_bytes()).expect("write");

        std::fs::create_dir_all(dir.join("tests")).expect("tests dir");
        let mut file = std::fs::File::create(dir.join("tests/main_test.lua")).expect("test");
        file.write_all(test.as_bytes()).expect("write");

        let config: ScriptConfig = serde_json::from_str(
            r#"{"id":"00000000-0000-0000-0000-000000000000",
                "entryPoint":"main.lua","includes":["**/*.lua"],
                "ignore":["tests/**"]}"#,
        )
        .expect("config parses");
        let mut config = config;
        config.project_path = Some(dir.to_path_buf());
        config
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_passing_suite_counts_its_cases() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = project(
            dir.path(),
            r#"
            local visits = kv "visits"
            on "fetch" (function(request)
                visits:set("last", request.path)
                return { body = visits:get("last") }
            end)
            "#,
            r#"
            test("the handler round-trips through kv", function()
                local response = fetch({ path = "/hello" })
                assert(response.body == "/hello", "kv did not round-trip")
            end)
            "#,
        );

        let summary = run_tests(&config).await.expect("suite runs");
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_assertion_is_a_failure_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = project(
            dir.path(),
            r#"on "fetch" (function() return { body = "actual" } end)"#,
            r#"
            test("expects something else", function()
                local response = fetch({})
                assert(response.body == "expected", "bodies differ")
            end)
            test("still runs after the failure", function() end)
            "#,
        );

        let summary = run_tests(&config).await.expect("suite runs");
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn secrets_decrypt_through_the_real_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = project(
            dir.path(),
            r#"
            local token = secret "api-token"
            on "fetch" (function() return { body = token } end)
            "#,
            r#"
            test("the secret reaches the handler", function()
                assert(fetch({}).body == "hunter2", "secret did not decrypt")
            end)
            "#,
        );
        std::fs::write(
            dir.path().join("tests/secrets.json"),
            r#"{"api-token":"hunter2"}"#,
        )
        .expect("secrets file");

        let summary = run_tests(&config).await.expect("suite runs");
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 0);
    }
}
