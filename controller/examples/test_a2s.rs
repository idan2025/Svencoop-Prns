// Manual validation harness for the A2S query client against a real,
// already-running DS. Not a unit test (needs a live UDP server), just a
// quick way to eyeball real parsed output before trusting the protocol
// parsing.
use sc_rns_controller::a2s;

#[tokio::main]
async fn main() {
    let addr = "127.0.0.1:27015".parse().unwrap();
    match a2s::query(addr).await {
        Ok(stats) => {
            println!("=== A2S_INFO ===");
            println!("{:#?}", stats.info);
            println!("=== players ({}) ===", stats.players_list.len());
            println!("{:#?}", stats.players_list);
        }
        Err(e) => println!("query failed: {e:#}"),
    }
}
