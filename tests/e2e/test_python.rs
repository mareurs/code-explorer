use crate::e2e::harness::run_all_expectations;

#[tokio::test]
async fn python_e2e() {
    run_all_expectations(
        "python",
        &["core-expectations.toml", "python-extensions.toml"],
    )
    .await;
}
