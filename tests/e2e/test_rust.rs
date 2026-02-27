use crate::e2e::harness::run_all_expectations;

#[tokio::test]
async fn rust_e2e() {
    run_all_expectations("rust", &["core-expectations.toml", "rust-extensions.toml"]).await;
}
