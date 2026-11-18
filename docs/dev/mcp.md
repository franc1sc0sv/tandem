# MCP y CLI de túneles

`tandem` es un solo binario (crate `tandem-cli`) con dos caras que comparten la misma tabla de herramientas y la misma API sobre la IPC del agente local: `tandem mcp` (servidor MCP por stdio) y `tandem <herramienta>` (CLI). El agente (`tandemd`) debe estar corriendo; ambos se conectan a su socket como `Peer::Client` (solo el usuario puede entrar, DESIGN §11).

## Agregarlo a Claude Code

```
claude mcp add tandem -- tandem mcp
```

Si el agente usa otro directorio de datos: `claude mcp add tandem -- tandem --data-dir <dir> mcp`. El asistente de la app (paso 6) muestra este comando con botón de copiar.

## Herramientas

`machines`, `resources`, `tunnels`, `tunnel_open`, `tunnel_close`, `exec`, `processes`, `output`, `kill`, `files_put`, `files_get`, `port_forward`, `port_stop`, `bring_back`. Cada una trae su descripción en español e inglés (`tandem --help` las lista con sus parámetros).

- Los ids de túnel son texto hex (los devuelve `tunnel_open`); los de proceso, números.
- `exec` no espera: devuelve el proceso. `output` lee desde un punto (`next` de la lectura anterior) y con `wait_ms` espera salida nueva o el fin del proceso.
- `tunnel_open` sin `title` usa el nombre del proyecto; el proyecto sale de la carpeta actual (o de `project`/`project_root`) y la sesión de `CLAUDE_SESSION_ID`. Con `workspace` el repo viaja con el túnel y vuelve con `bring_back` como rama `tandem/<nombre>`.

## Ejemplos

```
tandem tunnel-open --workspace --title "tests en la Mac"
tandem exec --tunnel 3fa2... --cmd cargo --args test --args --workspace
tandem output --tunnel 3fa2... --process 1 --wait-ms 5000
tandem bring-back --tunnel 3fa2... --name arreglo
tandem tunnel-close --tunnel 3fa2...
```

`--json` imprime el mismo JSON que devuelve el MCP (contrato estable); sin él, texto legible. En `output`, stdout y stderr del proceso van a los suyos y el estado (con el punto desde el que seguir) a stderr.
