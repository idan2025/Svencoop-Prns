// Integration test: does ds_start actually trigger a steamcmd pull?
use sc_rns_controller::{BridgeController, DsStartArgs};
use std::path::PathBuf;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let bundle = PathBuf::from("/tmp/opencode/ds-test");
    let _ = std::fs::remove_dir_all(&bundle);
    std::fs::create_dir_all(&bundle).unwrap();
    let mut ctrl = BridgeController::new(bundle.clone());

    println!("=== calling ds_start ===");
    let args = DsStartArgs {
        port: 27015,
        maxplayers: 8,
        map: "svencoop1".to_string(),
        install_dir: None,
    };
    match ctrl.ds_start(args).await {
        Ok(()) => println!("ds_start returned Ok (background task spawned)"),
        Err(e) => {
            println!("ds_start returned Err: {e}");
            return;
        }
    }

    // Poll state for 30s to see if the pull starts.
    use sc_rns_controller::DsStatus;
    let _ = DsStatus::default();
    for i in 1..=15 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let state = ctrl.state().await.unwrap();
        println!(
            "poll {i}: phase={} pct={:?} line={:?} running={}",
            serde_json::to_string(&state.ds.phase).unwrap_or_default(),
            state.ds.progress_pct,
            state.ds.last_line,
            state.ds.running
        );
    }
    let _ = std::fs::remove_dir_all(&bundle);
}