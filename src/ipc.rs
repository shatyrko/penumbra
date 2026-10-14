//! D-Bus: how `penumbra toggle`, `penumbra brightness +5` and friends reach the running app.

use zbus::blocking::Connection;

use crate::commands::{execute, Command, Response};
use crate::hub::Hub;

pub const NAME: &str = "dev.alecsh.Penumbra";
pub const PATH: &str = "/dev/alecsh/Penumbra";
pub const IFACE: &str = "dev.alecsh.Penumbra1";

/// Messages for the user interface thread.
#[derive(Debug, Clone)]
pub enum UiMsg {
    Toggle,
    Show,
    Hide,
    Settings(String),
    Quit,
    /// A command ran from the command line: show the on-screen indicator for it.
    Osd { display: String },
}

struct Service {
    hub: Hub,
    ui: async_channel::Sender<UiMsg>,
}

#[zbus::interface(name = "dev.alecsh.Penumbra1")]
impl Service {
    fn run(&self, json: String) -> String {
        let resp = match serde_json::from_str::<Command>(&json) {
            Ok(cmd) => {
                let osd = match &cmd {
                    Command::Brightness { display, .. } => Some(display.clone()),
                    _ => None,
                };
                let r = execute(&self.hub, cmd);
                if let (Some(d), true) = (osd, r.ok) {
                    let _ = self.ui.try_send(UiMsg::Osd { display: d });
                }
                r
            }
            Err(e) => Response { ok: false, message: format!("bad command: {e}"), data: serde_json::Value::Null },
        };
        serde_json::to_string(&resp).unwrap_or_default()
    }

    fn toggle(&self) {
        let _ = self.ui.try_send(UiMsg::Toggle);
    }

    fn show(&self) {
        let _ = self.ui.try_send(UiMsg::Show);
    }

    fn hide(&self) {
        let _ = self.ui.try_send(UiMsg::Hide);
    }

    fn settings(&self, page: String) {
        let _ = self.ui.try_send(UiMsg::Settings(page));
    }

    fn quit(&self) {
        let _ = self.ui.try_send(UiMsg::Quit);
    }
}

/// Claim the name and serve. Errors when another instance already runs.
pub fn serve(hub: Hub, ui: async_channel::Sender<UiMsg>) -> Result<Connection, String> {
    zbus::blocking::connection::Builder::session()
        .map_err(|e| e.to_string())?
        .name(NAME)
        .map_err(|e| e.to_string())?
        .serve_at(PATH, Service { hub, ui })
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())
}

fn call(method: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) -> Result<zbus::Message, String> {
    let conn = Connection::session().map_err(|e| e.to_string())?;
    conn.call_method(Some(NAME), PATH, Some(IFACE), method, body).map_err(|e| e.to_string())
}

/// Is the app running?
pub fn running() -> bool {
    let Ok(conn) = Connection::session() else { return false };
    let Ok(proxy) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return false };
    proxy.name_has_owner(NAME.try_into().unwrap()).unwrap_or(false)
}

pub fn send_command(cmd: &Command) -> Result<Response, String> {
    let json = serde_json::to_string(cmd).map_err(|e| e.to_string())?;
    let reply = call("Run", &(json,))?;
    let text: String = reply.body().deserialize().map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub fn send_simple(method: &str) -> Result<(), String> {
    call(method, &()).map(|_| ())
}

pub fn send_settings(page: &str) -> Result<(), String> {
    call("Settings", &(page.to_string(),)).map(|_| ())
}
