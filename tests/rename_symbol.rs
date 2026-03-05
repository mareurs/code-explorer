//! Integration tests for rename_symbol across supported languages.
//!
//! Each test requires the corresponding LSP server installed:
//!   Rust: rust-analyzer  |  Python: pyright-langserver  |  TypeScript: typescript-language-server
//!   Java: jdtls          |  Kotlin: kotlin-lsp
//!
//! Run by language:
//!   cargo test --test rename_symbol rename_rust -- --ignored
//!   cargo test --test rename_symbol rename_python -- --ignored
//!   cargo test --test rename_symbol -- --ignored   # all

use codescout::agent::Agent;
use codescout::lsp::LspManager;
use codescout::tools::symbol::RenameSymbol;
use codescout::tools::{Tool, ToolContext};
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;

async fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, ToolContext) {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    let ctx = ToolContext {
        agent,
        lsp: Arc::new(LspManager::new()),
        output_buffer: Arc::new(codescout::tools::output_buffer::OutputBuffer::new(20)),
        progress: None,
    };
    (dir, ctx)
}

/// Warm up an LSP server by opening a file and waiting for symbols.
///
/// Triggers `textDocument/didOpen` via ListSymbols, then polls until the
/// server returns actual symbols (proving it has parsed the file).
async fn warmup(ctx: &ToolContext, path: &str) {
    use codescout::tools::symbol::ListSymbols;
    let input = json!({ "path": path });
    for _ in 0u32..60 {
        match ListSymbols.call(input.clone(), ctx).await {
            Ok(v) if v["symbols"].as_array().map(|a| a.len()).unwrap_or(0) > 0 => return,
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Call rename_symbol with LSP warmup and retry for transient errors.
///
/// Opens the target file via LSP, waits for indexing, then attempts the
/// rename with retry for transient errors. For cross-file renames, call
/// `warmup()` on all involved files BEFORE calling this function.
async fn rename(
    ctx: &ToolContext,
    name_path: &str,
    path: &str,
    new_name: &str,
) -> serde_json::Value {
    // Phase 1: Warm up LSP on target file, then wait for project-wide indexing
    warmup(ctx, path).await;
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Phase 2: Attempt the rename with retry for transient errors
    let input = json!({ "name_path": name_path, "path": path, "new_name": new_name });
    for attempt in 0u32..10 {
        match RenameSymbol.call(input.clone(), ctx).await {
            Ok(v) => return v,
            Err(e) => {
                let msg = e.to_string();
                let transient = msg.contains("No references found")
                    || msg.contains("content modified")
                    || msg.contains("waiting for cargo")
                    || msg.contains("not yet ready");
                if transient && attempt < 9 {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        1000 * (attempt as u64 + 1),
                    ))
                    .await;
                    continue;
                }
                panic!("rename_symbol failed after {attempt} attempts: {e}");
            }
        }
    }
    unreachable!()
}

fn read(dir: &std::path::Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

const CARGO_TOML: &str = "\
[package]\n\
name = \"testpkg\"\n\
version = \"0.1.0\"\n\
edition = \"2021\"\n";

// ── Rust (rust-analyzer) ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires rust-analyzer"]
async fn rename_rust_function() {
    let (dir, ctx) = project(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/main.rs",
            "fn greet() -> &'static str { \"hello\" }\nfn main() { println!(\"{}\", greet()); }\n",
        ),
    ])
    .await;

    let r = rename(&ctx, "greet", "src/main.rs", "welcome").await;
    assert_eq!(r["files_changed"], 1);

    let content = read(dir.path(), "src/main.rs");
    assert!(
        content.contains("fn welcome()"),
        "function not renamed: {content}"
    );
    assert!(
        content.contains("welcome()"),
        "call site not renamed: {content}"
    );
    assert!(
        !content.contains("greet"),
        "old name still present: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires rust-analyzer"]
async fn rename_rust_struct_method() {
    let (dir, ctx) = project(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/main.rs",
            "\
struct Counter { value: i32 }
impl Counter {
    fn increment(&mut self) { self.value += 1; }
}
fn main() {
    let mut c = Counter { value: 0 };
    c.increment();
    c.increment();
}
",
        ),
    ])
    .await;

    // LSP uses "impl Counter" as the container name, not "Counter"
    let r = rename(&ctx, "impl Counter/increment", "src/main.rs", "advance").await;
    assert_eq!(r["files_changed"], 1);

    let content = read(dir.path(), "src/main.rs");
    assert!(
        content.contains("fn advance("),
        "method not renamed: {content}"
    );
    assert!(
        content.contains("c.advance()"),
        "call site not renamed: {content}"
    );
    assert!(
        !content.contains("increment"),
        "old name still present: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires rust-analyzer"]
async fn rename_rust_cross_file() {
    let (dir, ctx) = project(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub fn compute(x: i32) -> i32 { x * 2 }\n"),
        (
            "src/main.rs",
            "fn main() { let r = testpkg::compute(21); println!(\"{r}\"); }\n",
        ),
    ])
    .await;

    warmup(&ctx, "src/lib.rs").await;
    warmup(&ctx, "src/main.rs").await;
    let r = rename(&ctx, "compute", "src/lib.rs", "transform").await;
    assert!(
        r["files_changed"].as_u64().unwrap() >= 2,
        "expected cross-file rename: {r:?}"
    );

    let lib = read(dir.path(), "src/lib.rs");
    let main = read(dir.path(), "src/main.rs");
    assert!(lib.contains("fn transform("), "lib.rs not renamed: {lib}");
    assert!(main.contains("transform("), "main.rs not renamed: {main}");
    assert!(!lib.contains("compute"), "old name in lib.rs: {lib}");
}

// ── Python (pyright-langserver) ──────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires pyright-langserver"]
async fn rename_python_function() {
    let (dir, ctx) = project(&[(
        "main.py",
        "def greet():\n    return \"hello\"\n\nresult = greet()\nprint(greet())\n",
    )])
    .await;

    let r = rename(&ctx, "greet", "main.py", "welcome").await;
    assert_eq!(r["files_changed"], 1);

    let content = read(dir.path(), "main.py");
    assert!(
        content.contains("def welcome()"),
        "function not renamed: {content}"
    );
    assert!(
        !content.contains("greet"),
        "old name still present: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires pyright-langserver"]
async fn rename_python_class_method() {
    let (dir, ctx) = project(&[(
        "main.py",
        "\
class Dog:
    def speak(self):
        return \"woof\"

d = Dog()
print(d.speak())
result = d.speak()
",
    )])
    .await;

    let r = rename(&ctx, "Dog/speak", "main.py", "bark").await;
    assert_eq!(r["files_changed"], 1);

    let content = read(dir.path(), "main.py");
    assert!(
        content.contains("def bark("),
        "method not renamed: {content}"
    );
    assert!(
        content.contains("d.bark()"),
        "call site not renamed: {content}"
    );
    assert!(
        !content.contains("speak"),
        "old name still present: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires pyright-langserver"]
async fn rename_python_cross_file() {
    let (dir, ctx) = project(&[
        ("utils.py", "def compute(x):\n    return x * 2\n"),
        (
            "main.py",
            "from utils import compute\n\nresult = compute(21)\n",
        ),
    ])
    .await;

    warmup(&ctx, "utils.py").await;
    warmup(&ctx, "main.py").await;
    let r = rename(&ctx, "compute", "utils.py", "transform").await;
    assert!(
        r["files_changed"].as_u64().unwrap() >= 2,
        "expected cross-file rename: {r:?}"
    );

    let utils = read(dir.path(), "utils.py");
    let main = read(dir.path(), "main.py");
    assert!(
        utils.contains("def transform("),
        "utils.py not renamed: {utils}"
    );
    assert!(main.contains("transform"), "main.py not renamed: {main}");
}

// ── TypeScript (typescript-language-server) ───────────────────────────

const TSCONFIG: &str = r#"{"compilerOptions":{"target":"es2020","module":"commonjs","strict":true},"include":["**/*.ts"]}"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires typescript-language-server"]
async fn rename_ts_function() {
    let (dir, ctx) = project(&[
        ("tsconfig.json", TSCONFIG),
        (
            "main.ts",
            "function greet(): string { return \"hello\"; }\nconst r = greet();\nconsole.log(greet());\n",
        ),
    ])
    .await;

    let r = rename(&ctx, "greet", "main.ts", "welcome").await;
    assert_eq!(r["files_changed"], 1);

    let content = read(dir.path(), "main.ts");
    assert!(
        content.contains("function welcome()"),
        "function not renamed: {content}"
    );
    assert!(
        !content.contains("greet"),
        "old name still present: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires typescript-language-server"]
async fn rename_ts_class_method() {
    let (dir, ctx) = project(&[
        ("tsconfig.json", TSCONFIG),
        (
            "main.ts",
            "\
class Calculator {
    add(a: number, b: number): number { return a + b; }
}
const calc = new Calculator();
const result = calc.add(1, 2);
console.log(calc.add(3, 4));
",
        ),
    ])
    .await;

    let r = rename(&ctx, "Calculator/add", "main.ts", "sum").await;
    assert_eq!(r["files_changed"], 1);

    let content = read(dir.path(), "main.ts");
    assert!(
        content.contains("sum(a:") || content.contains("sum(a,"),
        "method not renamed: {content}"
    );
    assert!(
        content.contains("calc.sum("),
        "call site not renamed: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires typescript-language-server"]
async fn rename_ts_cross_file() {
    let (dir, ctx) = project(&[
        ("tsconfig.json", TSCONFIG),
        (
            "utils.ts",
            "export function compute(x: number): number { return x * 2; }\n",
        ),
        (
            "app.ts",
            "import { compute } from './utils';\nconst r = compute(21);\n",
        ),
    ])
    .await;

    warmup(&ctx, "utils.ts").await;
    warmup(&ctx, "app.ts").await;
    let r = rename(&ctx, "compute", "utils.ts", "transform").await;
    assert!(
        r["files_changed"].as_u64().unwrap() >= 2,
        "expected cross-file rename: {r:?}"
    );

    let utils = read(dir.path(), "utils.ts");
    let app = read(dir.path(), "app.ts");
    assert!(
        utils.contains("function transform("),
        "utils.ts not renamed: {utils}"
    );
    assert!(app.contains("transform"), "app.ts not renamed: {app}");
}

// ── Java (jdtls) ────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires jdtls"]
async fn rename_java_method() {
    let (dir, ctx) = project(&[(
        "Main.java",
        "\
public class Main {
    public static int greet() { return 42; }
    public static void main(String[] args) {
        System.out.println(greet());
    }
}
",
    )])
    .await;

    let r = rename(&ctx, "Main/greet()", "Main.java", "welcome").await;
    assert_eq!(r["files_changed"], 1);

    let content = read(dir.path(), "Main.java");
    assert!(
        content.contains("welcome()"),
        "method not renamed: {content}"
    );
    assert!(
        !content.contains("greet"),
        "old name still present: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires jdtls"]
async fn rename_java_instance_method() {
    let (dir, ctx) = project(&[(
        "Counter.java",
        "\
public class Counter {
    private int count = 0;
    public void increment() { count++; }
    public int getCount() { return count; }
    public static void main(String[] args) {
        Counter c = new Counter();
        c.increment();
        c.increment();
        System.out.println(c.getCount());
    }
}
",
    )])
    .await;

    let r = rename(&ctx, "Counter/increment()", "Counter.java", "advance").await;
    assert_eq!(r["files_changed"], 1);

    let content = read(dir.path(), "Counter.java");
    assert!(
        content.contains("void advance()"),
        "method not renamed: {content}"
    );
    assert!(
        content.contains("c.advance()"),
        "call site not renamed: {content}"
    );
    assert!(
        !content.contains("increment"),
        "old name still present: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires jdtls"]
async fn rename_java_cross_file() {
    let (dir, ctx) = project(&[
        // Eclipse project files — jdtls needs these to resolve cross-file references.
        (
            ".classpath",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<classpath>\n  <classpathentry kind=\"src\" path=\".\"/>\n  <classpathentry kind=\"output\" path=\"bin\"/>\n</classpath>\n",
        ),
        (
            ".project",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<projectDescription>\n  <name>testpkg</name>\n  <buildSpec><buildCommand><name>org.eclipse.jdt.core.javabuilder</name></buildCommand></buildSpec>\n  <natures><nature>org.eclipse.jdt.core.javanature</nature></natures>\n</projectDescription>\n",
        ),
        (
            "Utils.java",
            "public class Utils {\n    public static int compute(int x) { return x * 2; }\n}\n",
        ),
        (
            "Main.java",
            "\
public class Main {
    public static void main(String[] args) {
        System.out.println(Utils.compute(21));
    }
}
",
        ),
    ])
    .await;

    // Warm up both files so jdtls opens and indexes them before rename
    warmup(&ctx, "Utils.java").await;
    warmup(&ctx, "Main.java").await;
    let r = rename(&ctx, "Utils/compute(int)", "Utils.java", "transform").await;
    assert!(
        r["files_changed"].as_u64().unwrap() >= 2,
        "expected cross-file rename: {r:?}"
    );

    let utils = read(dir.path(), "Utils.java");
    let main = read(dir.path(), "Main.java");
    assert!(
        utils.contains("transform("),
        "Utils.java not renamed: {utils}"
    );
    assert!(main.contains("transform("), "Main.java not renamed: {main}");
}

// ── Kotlin (kotlin-lsp) ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires kotlin-lsp"]
async fn rename_kotlin_function() {
    let (dir, ctx) = project(&[(
        "Main.kt",
        "fun greet(): String = \"hello\"\n\nfun main() {\n    println(greet())\n}\n",
    )])
    .await;

    let r = rename(&ctx, "greet", "Main.kt", "welcome").await;
    assert_eq!(r["files_changed"], 1);

    let content = read(dir.path(), "Main.kt");
    assert!(
        content.contains("fun welcome()"),
        "function not renamed: {content}"
    );
    assert!(
        !content.contains("greet"),
        "old name still present: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires kotlin-lsp"]
async fn rename_kotlin_class_method() {
    let (dir, ctx) = project(&[(
        "Main.kt",
        "\
class Counter(var value: Int = 0) {
    fun increment() { value++ }
}

fun main() {
    val c = Counter()
    c.increment()
    c.increment()
    println(c.value)
}
",
    )])
    .await;

    let r = rename(&ctx, "Counter/increment", "Main.kt", "advance").await;
    assert_eq!(r["files_changed"], 1);

    let content = read(dir.path(), "Main.kt");
    assert!(
        content.contains("fun advance()"),
        "method not renamed: {content}"
    );
    assert!(
        content.contains("c.advance()"),
        "call site not renamed: {content}"
    );
    assert!(
        !content.contains("increment"),
        "old name still present: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires kotlin-lsp"]
async fn rename_kotlin_cross_file() {
    let (dir, ctx) = project(&[
        ("Utils.kt", "fun compute(x: Int): Int = x * 2\n"),
        ("Main.kt", "fun main() {\n    println(compute(21))\n}\n"),
    ])
    .await;

    warmup(&ctx, "Utils.kt").await;
    warmup(&ctx, "Main.kt").await;
    let r = rename(&ctx, "compute", "Utils.kt", "transform").await;
    assert!(
        r["files_changed"].as_u64().unwrap() >= 2,
        "expected cross-file rename: {r:?}"
    );

    let utils = read(dir.path(), "Utils.kt");
    let main = read(dir.path(), "Main.kt");
    assert!(
        utils.contains("fun transform("),
        "Utils.kt not renamed: {utils}"
    );
    assert!(main.contains("transform("), "Main.kt not renamed: {main}");
}
