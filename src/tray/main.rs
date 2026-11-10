mod model;
use maprisma_hue_lights::{ICON, TITLE, WINDOW_BUS, script_path, summary};
use model::Action;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, mpsc};
use zbus::{
    Connection,
    object_server::SignalEmitter,
    zvariant::{OwnedObjectPath, OwnedValue, Value as Variant},
};

type Props = HashMap<String, OwnedValue>;
type Layout = (i32, Props, Vec<OwnedValue>);
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;
struct State {
    stats: serde_json::Value,
    menu: model::Item,
    revision: u32,
}
impl State {
    fn status(&self) -> &'static str {
        "Active"
    }
    fn tooltip(&self) -> String {
        summary(&self.stats)
    }
}
type Shared = Arc<Mutex<State>>;
#[derive(Clone)]
struct Menu {
    state: Shared,
    tx: mpsc::UnboundedSender<Action>,
}
fn owned<T: Into<Variant<'static>>>(value: T) -> OwnedValue {
    OwnedValue::try_from(value.into()).unwrap()
}
fn properties(item: &model::Item, names: &[String]) -> Props {
    let mut props = Props::new();
    props.insert("label".into(), owned(item.label.clone()));
    props.insert("enabled".into(), owned(item.enabled));
    props.insert("visible".into(), owned(true));
    if !item.children.is_empty() {
        props.insert("children-display".into(), owned("submenu"));
    }
    if item.separator {
        props.insert("type".into(), owned("separator"));
    }
    if !names.is_empty() {
        props.retain(|key, _| names.contains(key));
    }
    props
}
fn layout(item: &model::Item, depth: i32, names: &[String]) -> Layout {
    (
        item.id,
        properties(item, names),
        if depth == 0 {
            vec![]
        } else {
            item.children
                .iter()
                .map(|c| {
                    OwnedValue::try_from(Variant::new(layout(
                        c,
                        if depth < 0 { -1 } else { depth - 1 },
                        names,
                    )))
                    .unwrap()
                })
                .collect()
        },
    )
}
impl Menu {
    async fn dispatch(&self, id: i32, event: &str) -> bool {
        let s = self.state.lock().await;
        let menu = s.menu.clone();
        let Some(item) = menu.find(id) else {
            return false;
        };
        if event == "clicked" {
            if let Some(action) = menu.activate(item.id) {
                let _ = self.tx.send(action);
            }
        }
        true
    }
}
#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        vec![]
    }
    async fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, Layout)> {
        let s = self.state.lock().await;
        let menu = s.menu.clone();
        let item = menu
            .find(parent_id)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("Unknown menu ID".into()))?;
        Ok((s.revision, layout(item, recursion_depth, &property_names)))
    }
    async fn get_group_properties(
        &self,
        ids: Vec<i32>,
        property_names: Vec<String>,
    ) -> Vec<(i32, Props)> {
        let s = self.state.lock().await;
        let menu = s.menu.clone();
        ids.iter()
            .filter_map(|id| {
                menu.find(*id)
                    .map(|i| (*id, properties(i, &property_names)))
            })
            .collect()
    }
    async fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        let s = self.state.lock().await;
        let menu = s.menu.clone();
        menu.find(id)
            .and_then(|i| properties(i, &[]).remove(&name))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("Unknown menu property".into()))
    }
    async fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        self.dispatch(id, &event_id).await;
    }
    async fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        let mut errors = vec![];
        for (id, event, _, _) in events {
            if !self.dispatch(id, &event).await {
                errors.push(id);
            }
        }
        errors
    }
    async fn about_to_show(&self, _id: i32) -> bool {
        false
    }
    async fn about_to_show_group(&self, ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        let s = self.state.lock().await;
        let menu = s.menu.clone();
        (
            vec![],
            ids.into_iter()
                .filter(|id| menu.find(*id).is_none())
                .collect(),
        )
    }
    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}
struct Tray {
    state: Shared,
    tx: mpsc::UnboundedSender<Action>,
}
#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Tray {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "maprisma-hue-lights"
    }
    #[zbus(property)]
    fn title(&self) -> &str {
        TITLE
    }
    #[zbus(property)]
    async fn status(&self) -> &str {
        self.state.lock().await.status()
    }
    #[zbus(property)]
    fn window_id(&self) -> u32 {
        0
    }
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ICON
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        vec![]
    }
    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Pixmaps {
        vec![]
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ICON
    }
    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Pixmaps {
        vec![]
    }
    #[zbus(property)]
    fn attention_movie_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    async fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        (
            ICON.into(),
            vec![],
            TITLE.into(),
            self.state.lock().await.tooltip(),
        )
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/MenuBar").unwrap()
    }
    fn activate(&self, _x: i32, _y: i32) {
        let _ = self.tx.send(Action::Activate);
    }
    fn secondary_activate(&self, _x: i32, _y: i32) {
        let _ = self.tx.send(Action::Open);
    }
    fn context_menu(&self, _x: i32, _y: i32) {} // The host displays the exported Menu.
    fn scroll(&self, _delta: i32, _orientation: String) {}
    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}
async fn execute(connection: &Connection, action: Action) {
    let dbus = zbus::fdo::DBusProxy::new(connection).await.unwrap();
    if let Ok(owner) = dbus.get_name_owner(WINDOW_BUS.try_into().unwrap()).await {
        if let Ok(proxy) = zbus::Proxy::new(
            connection,
            owner.as_str(),
            "/Window",
            "io.github.ubruckhaus.HueLights.Window",
        )
        .await
        {
            let method = if action == Action::Activate {
                "Close"
            } else {
                "Show"
            };
            let _ = proxy.call::<_, _, ()>(method, &()).await;
        }
    } else if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if let Ok(mut child) = tokio::process::Command::new(dir.join("maprisma-hue-lights"))
                .arg("--window")
                .spawn()
            {
                // Reap windows started by us; quitting the tray leaves the window open.
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
            }
        }
    }
}
async fn monitor(state: Shared, connection: Connection) {
    loop {
        if let Some(script) = script_path() {
            if let Ok(mut child) = tokio::process::Command::new("python3")
                .arg("-I")
                .arg(script)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
            {
                let _stdin = child.stdin.take(); // Keep open; default helper polling is 20 seconds.
                let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(stats) = serde_json::from_str::<serde_json::Value>(&line) {
                        let mut s = state.lock().await;
                        let before = s.tooltip();
                        s.stats = stats;
                        let changed = before != s.tooltip();
                        drop(s);
                        if changed {
                            let _ = Tray::new_tool_tip(
                                &SignalEmitter::new(&connection, "/StatusNotifierItem").unwrap(),
                            )
                            .await;
                        }
                    }
                }
                let _ = child.kill().await;
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(Mutex::new(State {
        stats: serde_json::Value::Null,
        menu: model::menu(),
        revision: 1,
    }));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let connection = zbus::connection::Builder::session()?
        .serve_at(
            "/StatusNotifierItem",
            Tray {
                state: state.clone(),
                tx: tx.clone(),
            },
        )?
        .serve_at(
            "/MenuBar",
            Menu {
                state: state.clone(),
                tx,
            },
        )?
        .build()
        .await?;
    // One tray per session: a second one ends here (zbus would otherwise
    // queue the name request and keep running).
    if connection
        .request_name_with_flags(
            "io.github.ubruckhaus.CosmicMaprismaHueLights.Tray",
            zbus::fdo::RequestNameFlags::DoNotQueue.into(),
        )
        .await
        .is_err()
    {
        return Ok(());
    }
    // Checking the unique owner also handles Plasma starting after us and watcher restarts.
    let watcher_connection = connection.clone();
    tokio::spawn(async move {
        let mut registered = String::new();
        loop {
            let dbus = zbus::fdo::DBusProxy::new(&watcher_connection).await;
            let owner = match dbus {
                Ok(p) => p
                    .get_name_owner("org.kde.StatusNotifierWatcher".try_into().unwrap())
                    .await
                    .ok()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                Err(_) => String::new(),
            };
            if owner.is_empty() {
                registered.clear();
            } else if owner != registered {
                if let Ok(proxy) = zbus::Proxy::new(
                    &watcher_connection,
                    owner.as_str(),
                    "/StatusNotifierWatcher",
                    "org.kde.StatusNotifierWatcher",
                )
                .await
                {
                    if proxy
                        .call::<_, _, ()>(
                            "RegisterStatusNotifierItem",
                            &(watcher_connection.unique_name().unwrap().as_str(),),
                        )
                        .await
                        .is_ok()
                    {
                        registered = owner;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
    let monitor_task = tokio::spawn(monitor(state.clone(), connection.clone()));
    let mut language = maprisma_hue_lights::i18n::setting();
    let mut refresh = tokio::time::interval(Duration::from_secs(1));
    loop {
        let action = tokio::select! {
            action = rx.recv() => match action { Some(a) => a, None => break },
            _ = refresh.tick() => {
                let current = maprisma_hue_lights::i18n::setting();
                if current != language {
                    language = current;
                    let mut s = state.lock().await;
                    s.menu = model::menu();
                    s.revision += 1;
                    let _ = Menu::layout_updated(&SignalEmitter::new(&connection, "/MenuBar")?, s.revision, 0).await;
                    let _ = Tray::new_tool_tip(&SignalEmitter::new(&connection, "/StatusNotifierItem")?).await;
                }
                continue;
            }
        };
        if action == Action::Quit {
            monitor_task.abort();
            return Ok(());
        }
        if let Action::Language(i) = action {
            if let Some((code, _)) = maprisma_hue_lights::i18n::choices().get(i) {
                if maprisma_hue_lights::i18n::save(code).is_ok() {
                    language = code.clone();
                    let mut s = state.lock().await;
                    s.menu = model::menu();
                    s.revision += 1;
                    let _ = Menu::layout_updated(&SignalEmitter::new(&connection, "/MenuBar")?, s.revision, 0).await;
                    let _ = Tray::new_tool_tip(&SignalEmitter::new(&connection, "/StatusNotifierItem")?).await;
                } else {
                    let mut s = state.lock().await;
                    s.menu = model::menu_error();
                    s.revision += 1;
                    let _ = Menu::layout_updated(&SignalEmitter::new(&connection, "/MenuBar")?, s.revision, 0).await;
                }
            }
        } else { execute(&connection, action).await; }
    }
    Ok(())
}
