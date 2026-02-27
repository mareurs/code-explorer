use crate::e2e::harness::run_all_expectations;

#[tokio::test]
async fn typescript_e2e() {
    run_all_expectations(
        "typescript",
        &["core-expectations.toml", "typescript-extensions.toml"],
    )
    .await;
}
