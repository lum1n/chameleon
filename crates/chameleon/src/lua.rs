//! The optional Lua runtime (`--features lua`): `~/.config/chameleon/init.lua`
//! registers hooks that run inside chameleon instead of as shell commands.
//!
//! ```lua
//! cham.hook {
//!   name = "ping", on = { "permission", "done" }, key = "ctrl-g",
//!   run = function(s)                     -- s is the hook payload as a table
//!     cham.notify(s.spec.harness .. " " .. (s.event or "key") .. ": " .. (s.spec.title or ""))
//!     local out = cham.exec(s.host, "uptime")   -- any script, on the session's host
//!     cham.log(out)
//!   end,
//! }
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chameleon_app::AppEvent;
use chameleon_app::config::HookConfig;
use mlua::{Function, Lua, LuaSerdeExt as _, Table, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::host::{self, Host};

/// Hook commands that name a Lua function instead of a shell command.
pub const PREFIX: &str = "lua:";

pub struct Runtime {
    lua: Lua,
    functions: HashMap<String, Function>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaRuntime")
            .field("hooks", &self.functions.len())
            .finish()
    }
}

/// The default script path, when it exists.
pub fn default_path() -> Option<std::path::PathBuf> {
    let cfg = chameleon_app::config::Config::default_path()?;
    let p = cfg.with_file_name("init.lua");
    p.is_file().then_some(p)
}

impl Runtime {
    /// Loads the script and returns the hooks it registered, to be added
    /// to the config before the app starts.
    pub fn load(
        path: &Path,
        hosts: HashMap<String, Arc<dyn Host>>,
        tx: UnboundedSender<AppEvent>,
    ) -> Result<(Self, Vec<HookConfig>), String> {
        let lua = Lua::new();
        let cham = lua.create_table().map_err(|e| e.to_string())?;
        let registered: Arc<std::sync::Mutex<Vec<(HookConfig, Function)>>> = Arc::default();

        let reg = registered.clone();
        let hook = lua
            .create_function(move |_, spec: Table| {
                let name: String = spec.get("name")?;
                let on: Option<Vec<String>> = spec.get("on")?;
                let key: Option<String> = spec.get("key")?;
                let run: Function = spec.get("run")?;
                let cfg = HookConfig {
                    name: name.clone(),
                    command: format!("{PREFIX}{name}"),
                    on: on.unwrap_or_default(),
                    key,
                    suspend_tui: false,
                };
                reg.lock().unwrap_or_else(|e| e.into_inner()).push((cfg, run));
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        cham.set("hook", hook).map_err(|e| e.to_string())?;

        let tx2 = tx.clone();
        let notify = lua
            .create_function(move |_, (text, error): (String, Option<bool>)| {
                let _ = tx2.send(AppEvent::Notice {
                    text,
                    error: error.unwrap_or(false),
                });
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        cham.set("notify", notify).map_err(|e| e.to_string())?;

        let log_fn = lua
            .create_function(|_, text: String| {
                log::info!(target: "lua", "{text}");
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        cham.set("log", log_fn).map_err(|e| e.to_string())?;

        let exec = lua
            .create_function(
                move |_, (host, script, stdin): (String, String, Option<String>)| {
                    let Some(h) = hosts.get(&host).cloned() else {
                        return Err(mlua::Error::runtime(format!("unknown host {host:?}")));
                    };
                    let exec = acc_core::Exec::with_stdin(script, stdin.unwrap_or_default().into_bytes());
                    let out = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(host::run(
                            h.as_ref(),
                            &exec,
                            Duration::from_secs(30),
                        ))
                    })
                    .map_err(mlua::Error::runtime)?;
                    Ok((out.stdout_text(), out.status.unwrap_or(-1), out.stderr_text()))
                },
            )
            .map_err(|e| e.to_string())?;
        cham.set("exec", exec).map_err(|e| e.to_string())?;

        lua.globals().set("cham", cham).map_err(|e| e.to_string())?;
        let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        lua.load(&src)
            .set_name(path.display().to_string())
            .exec()
            .map_err(|e| format!("{}: {e}", path.display()))?;

        let items = std::mem::take(&mut *registered.lock().unwrap_or_else(|e| e.into_inner()));
        let mut hooks = Vec::new();
        let mut functions = HashMap::new();
        for (cfg, f) in items {
            functions.insert(cfg.name.clone(), f);
            hooks.push(cfg);
        }
        Ok((Self { lua, functions }, hooks))
    }

    /// Runs a registered hook with the payload as a Lua table.
    pub fn call(&self, name: &str, payload: &serde_json::Value) -> Result<(), String> {
        let f = self
            .functions
            .get(name)
            .ok_or_else(|| format!("no lua hook {name:?}"))?;
        // JSON null becomes nil, so `s.event or "key"` works as expected.
        let opts = mlua::SerializeOptions::new()
            .serialize_none_to_null(false)
            .serialize_unit_to_null(false);
        let value: Value = self.lua.to_value_with(payload, opts).map_err(|e| e.to_string())?;
        f.call::<()>(value).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn registers_hooks_and_passes_null_as_nil() {
        let dir = std::env::temp_dir().join(format!("cham-lua-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("init.lua");
        std::fs::write(
            &script,
            r#"
            seen = {}
            cham.hook { name = "t", on = { "done" }, key = "ctrl-g", run = function(s)
              seen.event = s.event or "key"
              seen.title = s.spec.title
              local out, status = cham.exec("local", "printf hi")
              seen.out = out
              seen.status = status
              cham.notify("ok")
            end }
            "#,
        )
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut hosts: HashMap<String, Arc<dyn Host>> = HashMap::new();
        hosts.insert(
            "local".into(),
            Arc::new(crate::host::local::LocalHost::new("local")),
        );
        let (rt, hooks) = Runtime::load(&script, hosts, tx).expect("loads");
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].command, "lua:t");
        assert_eq!(hooks[0].key.as_deref(), Some("ctrl-g"));
        rt.call(
            "t",
            &serde_json::json!({"event": null, "spec": {"title": "x"}, "host": "local"}),
        )
        .expect("runs");
        let seen: Table = rt.lua.globals().get("seen").unwrap();
        assert_eq!(seen.get::<String>("event").unwrap(), "key");
        assert_eq!(seen.get::<String>("title").unwrap(), "x");
        assert_eq!(seen.get::<String>("out").unwrap(), "hi");
        assert_eq!(seen.get::<i32>("status").unwrap(), 0);
        assert!(matches!(rx.try_recv(), Ok(AppEvent::Notice { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
