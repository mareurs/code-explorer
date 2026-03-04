# Language Support

codescout provides three tiers of support depending on which backends are
available for a given language.

- **Full support** — LSP server + tree-sitter grammar. All symbol tools work,
  and richer AST extraction is available internally. Semantic search is also
  available after indexing.
- **LSP only** — LSP server configured, no tree-sitter grammar. Symbol
  navigation, references, and rename work.
- **Detection only** — Language is recognized for chunking and file detection.
  No LSP server and no tree-sitter grammar. Only file operations and semantic
  search (after indexing) are available.

---

## Supported Languages

| Language   | Extensions              | LSP Server                   | Support Level |
|------------|-------------------------|------------------------------|---------------|
| Rust       | `.rs`                   | `rust-analyzer`              | Full          |
| Python     | `.py`                   | `pyright-langserver`         | Full          |
| TypeScript | `.ts`                   | `typescript-language-server` | Full          |
| TSX        | `.tsx`                  | `typescript-language-server` | Full          |
| Go         | `.go`                   | `gopls`                      | Full          |
| Java       | `.java`                 | `jdtls`                      | Full          |
| Kotlin     | `.kt`, `.kts`           | `kotlin-language-server`     | Full          |
| JavaScript | `.js`                   | `typescript-language-server` | LSP only      |
| JSX        | `.jsx`                  | `typescript-language-server` | LSP only      |
| C          | `.c`                    | `clangd`                     | LSP only      |
| C++        | `.cpp`, `.cc`, `.cxx`   | `clangd`                     | LSP only      |
| C#         | `.cs`                   | `OmniSharp`                  | LSP only      |
| Ruby       | `.rb`                   | `solargraph`                 | LSP only      |

### Detection-Only Languages

These languages are recognized for chunking and file detection. No LSP server
is configured and no tree-sitter grammar is bundled.

| Language | Extensions       |
|----------|-----------------|
| PHP      | `.php`          |
| Swift    | `.swift`        |
| Scala    | `.scala`        |
| Elixir   | `.ex`, `.exs`   |
| Haskell  | `.hs`           |
| Lua      | `.lua`          |
| Bash     | `.sh`, `.bash`  |
| Markdown | `.md`           |

---

## Feature Matrix

| Feature                   | Full support | LSP only | Detection only |
|---------------------------|:------------:|:--------:|:--------------:|
| `find_symbol`             | Yes          | Yes      | No             |
| `list_symbols`            | Yes          | Yes      | No             |
| `find_references`         | Yes          | Yes      | No             |
| `replace_symbol`          | Yes          | Yes      | No             |
| `insert_code`             | Yes          | Yes      | No             |
| `rename_symbol`           | Yes          | Yes      | No             |
| `semantic_search`         | Yes          | Yes      | Yes            |
| File tools                | Yes          | Yes      | Yes            |

---

## Installing LSP Servers

codescout looks for each LSP server binary on `PATH`. The quickest way to
get started is the bundled install script:

```bash
# See what's installed and what's missing
./scripts/install-lsp.sh --check

# Install all supported LSP servers
./scripts/install-lsp.sh --all

# Install specific languages only
./scripts/install-lsp.sh rust python typescript go
```

The script supports Linux and macOS, detects your package managers, and skips
servers that are already installed. For manual installation, see the
per-language instructions below.

### Rust

```bash
rustup component add rust-analyzer
```

Binary: `rust-analyzer`

### Python

```bash
npm install -g pyright
```

Binary: `pyright-langserver`, invoked with `--stdio`.

### TypeScript, JavaScript, TSX, JSX

```bash
npm install -g typescript-language-server typescript
```

Binary: `typescript-language-server`, invoked with `--stdio`. One installation
covers TypeScript, JavaScript, TSX, and JSX.

### Go

```bash
go install golang.org/x/tools/gopls@latest
```

Binary: `gopls`. Ensure `$(go env GOPATH)/bin` is on `PATH`.

### Java

`jdtls` (Eclipse JDT Language Server) is distributed as a standalone archive
from the [Eclipse downloads page](https://download.eclipse.org/jdtls/). Unpack
and place the launcher script on `PATH`.

Binary: `jdtls`

### Kotlin

`kotlin-language-server` is distributed as a release archive on the
[GitHub releases page](https://github.com/fwcd/kotlin-language-server/releases).
Unpack and place the `kotlin-language-server` script on `PATH`.

Binary: `kotlin-language-server`

### C and C++

`clangd` is shipped with LLVM/Clang. Install via your system package manager:

```bash
# Debian/Ubuntu
sudo apt install clangd

# macOS
brew install llvm   # or: xcode-select --install

# Fedora/RHEL
sudo dnf install clang-tools-extra
```

Binary: `clangd`. One installation covers both C and C++.

### C#

`OmniSharp` is bundled with the .NET SDK or available as a standalone binary.
The standalone build can be downloaded from the
[OmniSharp releases page](https://github.com/OmniSharp/omnisharp-roslyn/releases).
Place the binary on `PATH`.

Binary: `OmniSharp` (note the capital O), invoked with `-lsp`.

### Ruby

```bash
gem install solargraph
```

Binary: `solargraph`, invoked with `stdio` (no leading `--`).

---

## Known Quirks

**jdtls** requires a data/workspace directory for project indexes. Some wrapper
scripts accept `--data` to specify this path. If symbol tools return empty
results, check whether jdtls started correctly by examining the server log.
The `JAVA_HOME` environment variable should point to a JDK 17+ installation.

**OmniSharp** binary name starts with a capital O (`OmniSharp`, not
`omnisharp`). On case-sensitive filesystems the name must match exactly. Some
distributions ship a lowercase alias; check with `which OmniSharp` before
assuming the server is unavailable.

**solargraph** takes a positional argument (`stdio`) rather than a flag
(`--stdio`). This differs from most other servers. The invocation is
`solargraph stdio`, not `solargraph --stdio`.

**kotlin-language-server** builds a project index on first startup, which can
take 30–120 seconds on a large project. Symbol tools will return empty results
until the index is ready. Subsequent startups are fast.

**typescript-language-server** handles JavaScript and JSX in addition to
TypeScript and TSX. The LSP `languageId` sent for TSX files is
`typescriptreact` and for JSX files is `javascriptreact` — this is handled
internally and requires no configuration.

---

## Overriding LSP Server Configuration

The defaults above can be overridden per-project in `.code-explorer/project.toml`.
See [Project Configuration](configuration/project-toml.md) for details.
