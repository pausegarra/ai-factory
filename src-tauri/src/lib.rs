mod lifecycle;
mod startup;

use factory_core::{Factory, FactoryEvent, LogChunk, RunDetail, Settings, Snapshot};
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, State,
};
use tauri_plugin_autostart::ManagerExt;

struct DesktopState {
    factory: Arc<Factory>,
    exit_gate: lifecycle::ExitGate,
}

async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> factory_core::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
async fn get_settings(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Settings, String> {
    let factory = state.factory.clone();
    let mut settings = blocking(move || factory.settings()).await?;
    settings.start_at_login = app
        .autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())?;
    Ok(settings)
}

#[tauri::command]
async fn save_settings(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
    settings: Settings,
) -> Result<Settings, String> {
    settings.validate().map_err(|error| error.to_string())?;
    let current = app
        .autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())?;
    let factory = state.factory.clone();
    let desired = settings.start_at_login;
    tauri::async_runtime::spawn_blocking(move || {
        startup::change_with_persistence(
            current,
            desired,
            |enabled| {
                if enabled {
                    app.autolaunch().enable()
                } else {
                    app.autolaunch().disable()
                }
                .map_err(|error| error.to_string())
            },
            || {
                factory
                    .save_settings(settings)
                    .map_err(|error| format!("{error:#}"))
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_agent_models(
    state: State<'_, DesktopState>,
    agent: String,
) -> Result<Vec<factory_core::AgentModel>, String> {
    state
        .factory
        .agent_models(&agent)
        .await
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
async fn get_snapshot(state: State<'_, DesktopState>) -> Result<Snapshot, String> {
    let factory = state.factory.clone();
    blocking(move || factory.snapshot()).await
}

#[tauri::command]
async fn get_run_detail(
    state: State<'_, DesktopState>,
    run_id: String,
) -> Result<RunDetail, String> {
    let factory = state.factory.clone();
    blocking(move || factory.run_detail(&run_id)).await
}

#[tauri::command]
async fn get_logs(
    state: State<'_, DesktopState>,
    run_id: String,
    node_id: String,
    offset: u64,
) -> Result<LogChunk, String> {
    let factory = state.factory.clone();
    blocking(move || factory.logs(&run_id, &node_id, offset)).await
}

#[tauri::command]
async fn cancel_run(state: State<'_, DesktopState>, run_id: String) -> Result<(), String> {
    let factory = state.factory.clone();
    blocking(move || factory.cancel(&run_id)).await
}

#[tauri::command]
async fn retry_run(state: State<'_, DesktopState>, run_id: String) -> Result<String, String> {
    let factory = state.factory.clone();
    blocking(move || factory.retry(&run_id)).await
}

#[tauri::command]
async fn poll_now(state: State<'_, DesktopState>) -> Result<(), String> {
    state
        .factory
        .poll_now()
        .await
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
async fn import_legacy(
    state: State<'_, DesktopState>,
    config_path: Option<String>,
    database_path: Option<String>,
) -> Result<factory_core::ImportReport, String> {
    let factory = state.factory.clone();
    blocking(move || {
        factory.import_legacy(config_path.map(Into::into), database_path.map(Into::into))
    })
    .await
}

fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn request_exit(app: &tauri::AppHandle) {
    let state = app.state::<DesktopState>();
    if !state.exit_gate.request_shutdown() {
        return;
    }
    let factory = state.factory.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        factory.shutdown().await;
        app.state::<DesktopState>().exit_gate.finish_shutdown();
        app.exit(0);
    });
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_window(app)
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let root = app.path().app_data_dir()?;
            // Debug smoke tests use isolated state; released apps always use OS application data.
            #[cfg(debug_assertions)]
            let root = std::env::var_os("AI_FACTORY_DATA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or(root);
            let factory = Factory::open(root)?;
            let mut events = factory.subscribe();
            app.manage(DesktopState {
                factory: factory.clone(),
                exit_gate: lifecycle::ExitGate::default(),
            });

            let show = MenuItem::with_id(app, "show", "Open AI Factory", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit AI Factory", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::new().menu(&menu).tooltip("AI Factory");
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.on_menu_event(|app, event| match event.id.as_ref() {
                "show" => show_window(app),
                "quit" => request_exit(app),
                _ => {}
            })
            .build(app)?;

            if std::env::args().any(|arg| arg == "--background") {
                if let Some(window) = app.get_webview_window("main") {
                    window.hide()?;
                }
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                factory.start();
                loop {
                    let event = match events.recv().await {
                        Ok(event) => event,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => FactoryEvent {
                            kind: "resync".into(),
                            run_id: None,
                            node_id: None,
                            timestamp: factory_core::now(),
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    };
                    let _ = handle.emit("factory:event", event);
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            get_agent_models,
            save_settings,
            get_snapshot,
            get_run_detail,
            get_logs,
            cancel_run,
            retry_run,
            poll_now,
            import_legacy
        ])
        .build(tauri::generate_context!())
        .expect("AI Factory could not start");
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            if !app.state::<DesktopState>().exit_gate.can_exit() {
                api.prevent_exit();
                request_exit(app);
            }
        }
    });
}
