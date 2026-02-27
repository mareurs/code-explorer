use crate::e2e::harness::run_all_expectations;

#[tokio::test]
async fn java_e2e() {
    run_all_expectations("java", &["core-expectations.toml", "java-extensions.toml"]).await;
}
