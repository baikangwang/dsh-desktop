// @dsh-desktop/dsh-ops — DSH-side routes the desktop shell needs:
//   GET  /api/health          readiness + status (pid/port/uptime)
//   POST /api/admin/shutdown  graceful, secret-guarded shutdown
//
// These are registered as EXACT routes on the webServer service, so they match
// BEFORE the `/api` PREFIX route owned by dsh-client-connection (see
// dsh-host-webserver match(): exact table first). They therefore bypass the
// `/api` browser-trust fence — which is why shutdown carries its own
// Authorization guard.

const name = "desktop-surface-ops";

// appExit is provided by the launcher (runProfile -> provideCmdline): a
// graceful-shutdown controller `(code) => void` that disposes the tree and
// sets process.exitCode. On Windows there is no signal path from a parent
// process to a Node child, so this endpoint is the in-band shutdown channel.
const inject = ["webServer", "appExit"];

const TOKEN_ENV = "DSH_DESKTOP_SHUTDOWN_TOKEN";

function writeJson(res, status, body) {
  const data = Buffer.from(JSON.stringify(body), "utf8");
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": String(data.length),
    "cache-control": "no-store",
  });
  res.end(data);
}

function apply(ctx, config) {
  // Per-boot secret the shell passes via env (or an explicit config.token).
  const token = (config && config.token) || process.env[TOKEN_ENV];

  ctx.effect(
    () =>
      ctx.webServer.register({
        kind: "exact",
        path: "/api/health",
        handler: async (req, res) => {
          if (req.method !== "GET" && req.method !== "HEAD") {
            return writeJson(res, 405, { error: "method not allowed" });
          }
          writeJson(res, 200, {
            status: "ok",
            pid: process.pid,
            uptimeSec: Math.round(process.uptime()),
            port: ctx.webServer.port,
            dshHome: process.env.DSH_HOME ?? null,
          });
        },
      }),
    "desktop-ops: /api/health",
  );

  ctx.effect(
    () =>
      ctx.webServer.register({
        kind: "exact",
        path: "/api/admin/shutdown",
        handler: async (req, res) => {
          if (req.method !== "POST") {
            return writeJson(res, 405, { error: "method not allowed" });
          }
          const auth = String(req.headers["authorization"] ?? "");
          if (!token || auth !== `Bearer ${token}`) {
            return writeJson(res, 401, { error: "unauthorized" });
          }
          writeJson(res, 202, { status: "shutting down" });
          // Flush the 202 before the tree disposes.
          setTimeout(() => ctx.appExit(0), 50);
        },
      }),
    "desktop-ops: /api/admin/shutdown",
  );
}

export { apply, inject, name };
