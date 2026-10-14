//! The application: tray icon, panel, settings window, on-screen indicators.

pub mod arrange;
pub mod controls;
pub mod osd;
pub mod panel;
pub mod settings;
pub mod tray;
pub mod widgets;

use std::cell::{Cell, RefCell};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio};

use crate::hub::{Hub, HubEvent, HubOptions};
use crate::ipc::{self, UiMsg};

pub fn monitor_for(name: &str) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let mons = display.monitors();
    for i in 0..mons.n_items() {
        if let Some(m) = mons.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()) {
            if m.connector().as_deref() == Some(name) {
                return Some(m);
            }
        }
    }
    None
}

pub fn run() -> ExitCode {
    if ipc::running() {
        eprintln!("Penumbra is already running; toggling its panel.");
        let _ = ipc::send_simple("Toggle");
        return ExitCode::SUCCESS;
    }
    let app = adw::Application::builder().application_id(ipc::NAME).flags(gio::ApplicationFlags::NON_UNIQUE).build();
    let started = Rc::new(Cell::new(false));
    app.connect_startup(|_| {
        gtk::Window::set_default_icon_name("penumbra");
        let provider = gtk::CssProvider::new();
        provider.load_from_string(include_str!("style.css"));
        if let Some(d) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&d, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
            // Icons shipped with the program (the app icon) are found even when it is not installed.
            let theme = gtk::IconTheme::for_display(&d);
            if let Ok(exe) = std::env::current_exe() {
                for up in ["../../data", "../share/penumbra", "../../../data"] {
                    if let Some(p) = exe.parent().map(|d| d.join(up)) {
                        if p.exists() {
                            theme.add_search_path(&p);
                        }
                    }
                }
            }
        }
    });
    let code = Rc::new(Cell::new(ExitCode::SUCCESS));
    {
        let started = started.clone();
        let code = code.clone();
        app.connect_activate(move |app| {
            if started.replace(true) {
                return;
            }
            if let Err(e) = setup(app) {
                eprintln!("penumbra: {e}");
                code.set(ExitCode::FAILURE);
                app.quit();
            }
        });
    }
    app.run_with_args::<&str>(&[]);
    code.get()
}

struct Shared {
    hub: Hub,
    panel: Rc<panel::Panel>,
    settings: Rc<settings::Settings>,
    osd: osd::Osd,
}

fn setup(app: &adw::Application) -> Result<(), String> {
    let mut opts = HubOptions::default();
    // Development: fake monitors attached to the named outputs, e.g. PENUMBRA_MOCK=PTEST-1
    if let Ok(list) = std::env::var("PENUMBRA_MOCK") {
        let conns: Vec<String> = list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        opts.backend = Some(std::sync::Arc::new(crate::ddc::Mock::new(&conns)));
    }
    let (hub, hub_rx) = Hub::start(opts)?;
    let (ui_tx, ui_rx) = async_channel::unbounded::<UiMsg>();
    let conn = ipc::serve(hub.clone(), ui_tx.clone()).map_err(|e| format!("cannot offer the D-Bus service (is Penumbra already running?): {e}"))?;
    // The connection must live as long as the app.
    std::mem::forget(conn);

    let gtk_app: &gtk::Application = app.upcast_ref();
    let panel = panel::Panel::new(gtk_app, &hub);
    let settings = settings::Settings::new(gtk_app, &hub);
    let osd = osd::Osd::new();
    let shared = Rc::new(Shared { hub: hub.clone(), panel: panel.clone(), settings: settings.clone(), osd });

    {
        let s = settings.clone();
        panel.connect_settings(move |page| s.show(page));
    }
    {
        let (hub, app) = (hub.clone(), app.clone());
        panel.connect_quit(move || {
            hub.shutdown();
            app.quit();
        });
    }

    let tray_handle = tray::spawn(tray::Tray { ui: ui_tx.clone(), hub: hub.clone(), profiles: hub.profile_names() });
    if tray_handle.is_none() {
        eprintln!("penumbra: no tray available; open the panel with `penumbra toggle`");
    }

    // messages from the tray, the command line and D-Bus
    {
        let (shared, app) = (shared.clone(), app.clone());
        glib::spawn_future_local(async move {
            while let Ok(msg) = ui_rx.recv().await {
                match msg {
                    UiMsg::Toggle => shared.panel.toggle(),
                    UiMsg::Show => shared.panel.show(),
                    UiMsg::Hide => shared.panel.hide(),
                    UiMsg::Settings(p) => shared.settings.show(&p),
                    UiMsg::Quit => {
                        shared.hub.shutdown();
                        app.quit();
                    }
                    UiMsg::Osd { display } => {
                        // Not while the panel itself is open: it shows the same slider.
                        if shared.panel.is_visible() {
                            continue;
                        }
                        let names = shared.hub.resolve(&display);
                        if let Some(v) = names.first().and_then(|n| shared.hub.view(n)) {
                            shared.osd.show(&v.name, &v.display_name(), v.brightness);
                        }
                    }
                }
            }
        });
    }

    // changes in the hub: refresh what is on screen, a moment later and only once
    {
        let shared = shared.clone();
        let pending = Rc::new(Cell::new(false));
        let last_profiles = Rc::new(RefCell::new(hub.profile_names()));
        let tray_handle = Rc::new(tray_handle);
        glib::spawn_future_local(async move {
            while let Ok(ev) = hub_rx.recv().await {
                match ev {
                    HubEvent::Changed => {
                        if !pending.replace(true) {
                            let (shared, pending, last, th) = (shared.clone(), pending.clone(), last_profiles.clone(), tray_handle.clone());
                            glib::timeout_add_local_once(Duration::from_millis(40), move || {
                                pending.set(false);
                                if shared.panel.is_visible() {
                                    shared.panel.sync(&shared.hub.views());
                                }
                                shared.settings.sync();
                                let names = shared.hub.profile_names();
                                if *last.borrow() != names {
                                    *last.borrow_mut() = names.clone();
                                    if let Some(h) = th.as_ref() {
                                        h.update(|t| t.profiles = names.clone());
                                    }
                                }
                            });
                        }
                    }
                    HubEvent::Notice(m) => {
                        shared.panel.toast(&m, false);
                        shared.settings.toast(&m, false);
                    }
                    HubEvent::Error(m) => {
                        shared.panel.toast(&m, true);
                        shared.settings.toast(&m, true);
                    }
                }
            }
        });
    }

    // stay alive without a window: the app lives in the tray
    std::mem::forget(app.hold());
    // the tray handle also has to live on
    Ok(())
}
