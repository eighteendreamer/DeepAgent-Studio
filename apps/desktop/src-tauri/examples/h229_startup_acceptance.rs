//! H229 real Wry setup at the production AppService::open/recovery boundary.
//! Called only by the owned synthetic CLI test fixture. No full desktop setup,
//! credential store, WebView, model request, plugin discovery or real user data.
//! Link the same Tauri-generated resource.lib as h97_exit_acceptance on Windows.
use deepagent_app_core::{AppService, ProjectService};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(
        args.len(),
        4,
        "requires synthetic database, good ID, bad ID and project"
    );
    let accepted = Arc::new(AtomicBool::new(false));
    let checked = accepted.clone();
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .setup(move |app| {
            // These are the same startup calls made by the real desktop setup,
            // before that setup constructs its OS credential store.
            let service = AppService::open(&args[0])?;
            assert!(service.recover_unfinished_runs()?.is_empty());
            ProjectService::new(service.shared_database()).add_project(&args[3])?;
            assert_eq!(service.list_sessions()?.len(), 1); // only registered A is visible
            assert_eq!(service.session_conversation(&args[1])?.len(), 1);
            assert!(service.session_conversation(&args[2]).is_err());
            let report = service.search_sessions_with_status("repairneedle", Some(&args[3]), 10)?;
            assert!(report.complete);
            assert_eq!(report.version, 1);
            assert_eq!(report.results.len(), 1);
            assert_eq!(report.results[0].session.id, args[1]);
            checked.store(true, Ordering::Release);
            tauri::WindowBuilder::new(app, "h229-synthetic")
                .visible(false)
                .build()?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                handle.exit(0);
            });
            Ok(())
        })
        .build(context)
        .unwrap();
    assert_eq!(app.run_return(|_, _| {}), 0);
    assert!(accepted.load(Ordering::Acquire));
    println!("H229 isolated Wry startup boundary accepted; no full desktop setup or credentials");
}
