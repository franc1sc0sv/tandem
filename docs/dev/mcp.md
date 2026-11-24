# Tunnels MCP and CLI

`tandem` is a single binary (crate `tandem-cli`) with two faces that share the same tool table and the same API over the local agent's IPC: `tandem mcp` (MCP server over stdio) and `tandem <tool>` (CLI). The agent (`tandemd`) must be running; both connect to its socket as `Peer::Client` (only the user can get in, DESIGN §11).

## Adding it to Claude Code

```
claude mcp add tandem -- tandem mcp
```

If the agent uses another data directory: `claude mcp add tandem -- tandem --data-dir <dir> mcp`. The app's setup (step 6) shows this command with a copy button.

## Tools

`machines`, `resources`, `tunnels`, `tunnel_open`, `tunnel_close`, `exec`, `processes`, `output`, `kill`, `files_put`, `files_get`, `port_forward`, `port_stop`, `bring_back`. Each one carries its description (`tandem --help` lists them with their parameters).

- Tunnel ids are hex text (returned by `tunnel_open`); process ids are numbers.
- `exec` does not wait: it returns the process. `output` reads from a point (`next` from the previous read) and with `wait_ms` waits for new output or for the process to end.
- `tunnel_open` without `title` uses the project name; the project comes from the current folder (or from `project`/`project_root`) and the session from `CLAUDE_SESSION_ID`. With `workspace` the repo travels with the tunnel and comes back with `bring_back` as a `tandem/<name>` branch.

## Examples

```
tandem tunnel-open --workspace --title "tests on the Mac"
tandem exec --tunnel 3fa2... --cmd cargo --args test --args --workspace
tandem output --tunnel 3fa2... --process 1 --wait-ms 5000
tandem bring-back --tunnel 3fa2... --name fix
tandem tunnel-close --tunnel 3fa2...
```

`--json` prints the same JSON the MCP returns (stable contract); without it, readable text. In `output`, the process's stdout and stderr go to their own and the status (with the point from which to continue) to stderr.

## Credentials (T-130)

The other machine never receives credentials:

- `files_put` and `tunnel_open { workspace }` do not send what looks like a credential (`.env`, `*.pem`, `id_rsa*`, `.ssh/`, `.aws/`, `.npmrc` with a token…), whether or not it is in `.gitignore`. The response carries `skipped` and a `warning`; the CLI warns on stderr. To send one, name it in `include_secrets` (exact path relative to `local` or to the repo).
- `exec` rejects `env` variables that look like secrets (`GH_TOKEN`, `AWS_*`, `*_KEY`…) unless `allow_secret_env: true` is set in that call. `SSH_AUTH_SOCK` and agents never pass.
- The workspace repo arrives without remotes or `credential.helper`; what is already committed travels with its history.
