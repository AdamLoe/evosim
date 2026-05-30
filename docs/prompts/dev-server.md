# Dev-server prompt

You need to build the wasm bundle, start (or restart) the Vite dev
server, and verify the sim is running threaded. Load both docs below
together; this task almost always needs the canonical build facts and
the procedural dev loop.

The canonical incantation, every flag, every verification step, and
every common failure mode lives in
[`../architecture/build-and-deploy.md`](../architecture/build-and-deploy.md).

The procedural inner loop (when to rebuild, when to hard-reload, what
the two `[sim] ...` log lines mean) lives in
[`../agent-context/dev-loop.md`](../agent-context/dev-loop.md).

Do not re-derive the commands here — drift would
silently produce a single-threaded sim that "looks fine."
