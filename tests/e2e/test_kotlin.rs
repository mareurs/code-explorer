use crate::e2e::harness::run_all_expectations;

#[tokio::test]
async fn kotlin_e2e() {
    run_all_expectations(
        "kotlin",
        &["core-expectations.toml", "kotlin-extensions.toml"],
    )
    .await;
}
