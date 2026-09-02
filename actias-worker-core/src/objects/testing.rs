//! Fixtures shared by the object tests and the tests of the loops
//! that drive objects.

use super::*;
use crate::proto::bundle::{Bundle, File};
use crate::proto::kv_service::kv_service_client::KvServiceClient;
use crate::proto::script_service::{Revision, Script};
use crate::runtime::PreparedRevision;

pub(crate) async fn runtime_with(source: &str) -> ActiasRuntime {
    runtime_with_files(&[("main.lua", source)]).await
}

/// Like [`runtime_with`] but with a whole bundle of files.
pub(crate) async fn runtime_with_files(files: &[(&str, &str)]) -> ActiasRuntime {
    let revision = Revision {
        bundle: Some(Bundle {
            entry_point: "main.lua".to_owned(),
            files: files
                .iter()
                .map(|(path, content)| File {
                    file_path: (*path).to_owned(),
                    content: content.as_bytes().to_vec(),
                    ..Default::default()
                })
                .collect(),
        }),
        ..Default::default()
    };
    let prepared =
        Arc::new(PreparedRevision::prepare(Script::default(), revision).expect("prepares"));

    let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
    let egress = crate::egress::EgressClient::new(crate::egress::EgressPolicy::new([], false))
        .expect("egress builds");

    let runtime = ActiasRuntime::new(
        prepared,
        KvServiceClient::new(crate::plain_grpc(channel)),
        egress,
        None,
        None,
        None,
    )
    .await
    .expect("runtime builds");

    // A real await point for the interleaving test: without the input
    // gate, a second call could run while the first sleeps here.
    runtime
        .globals()
        .set(
            "sleep_ms",
            runtime
                .create_async_function(|_, ms: u64| async move {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    Ok(())
                })
                .expect("function builds"),
        )
        .expect("global sets");

    runtime
}
