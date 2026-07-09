fn main() {
    let plan = token_station_router_core::RouterPlan::new("bootstrap");
    println!("token-station CLI bootstrap: {}", plan.name());
}
