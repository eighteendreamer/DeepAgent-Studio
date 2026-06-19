# deepagent-codegraph

Native code graph engine for DeepAgent Studio. The crate indexes a project once
into SQLite + FTS5 and reuses the same graph for two consumers:

- AI tools query the rich graph directly for symbols, source snippets, call
  chains, impact, node detail, and error locations.
- The desktop project-map panel receives a projected
  `.understand-anything/knowledge-graph.json` view for human navigation.

## Architecture

The pipeline is:

1. `scanner` walks the project with `.gitignore` support, source filtering, and
   BLAKE3 content hashes.
2. `extraction` parses grammar-backed languages with tree-sitter and uses a
   conservative generic extractor for additional recognised source languages.
3. `store` persists files, nodes, edges, unresolved references, and FTS rows in
   SQLite.
4. `resolution` connects imports and cross-file calls where possible.
5. `query` serves precise AI-facing answers from the stored graph.
6. `projection` down-projects the rich graph to the desktop project-map schema.

`CodeGraph::index_all` performs a full rebuild. `CodeGraph::sync` performs
incremental re-indexing from scanner content hashes. `watcher` contains the
deterministic path filtering and debounce core used to trigger sync after file
change bursts.

## Language Coverage

Grammar-backed extractors:

- Rust
- TypeScript / JavaScript
- Python
- Go
- Java, C#, C, C++, Ruby, PHP, Swift, Kotlin, Scala, Dart, Lua, Shell, CSS,
  and HTML

Additional recognised languages use `GenericExtractor` without a compiled
grammar yet, still extracting functions, classes or modules, imports, and
same-file calls:

- Elixir, Haskell, R, Julia, SQL, XML, Vue, and Svelte

Framework route recognition emits `route` nodes and `references` edges to
handlers for common Axum/Actix, Express, FastAPI, and Django patterns.

## AI Tools

`deepagent-builtins` exposes the code graph through read-only tools:

- `codegraph_search`
- `codegraph_explore`
- `codegraph_callers`
- `codegraph_callees`
- `codegraph_impact`
- `codegraph_node`
- `codegraph_locate`

The host bridge in `deepagent-app-core` opens the project graph and returns
structured JSON, including guidance when a project has not been indexed yet.

## Quality Gates

Run the focused codegraph gates offline:

```bash
cargo test -p deepagent-codegraph --offline
cargo clippy -p deepagent-codegraph --all-targets --offline -- -D warnings
```
