use super::*;
use crate::layout::{
    ScreenRect, find_results_layout, hotlist_layout, listbox_dialog_layout, tree_layout,
};

const VIEWPORT_WIDTH: u16 = 120;
const VIEWPORT_HEIGHT: u16 = 40;

fn viewport() -> ScreenRect {
    ScreenRect::new(0, 0, VIEWPORT_WIDTH, VIEWPORT_HEIGHT)
}

fn click_commands(app: &AppState, column: u16, row: u16) -> MouseClickCommands {
    app.commands_for_left_click(column, row, VIEWPORT_WIDTH, VIEWPORT_HEIGHT)
        .expect("list row should be clickable")
}

#[test]
fn find_result_click_selects_visible_row_and_offers_open_activation() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-find-mouse-{stamp}"));
    let second = root.join("second");
    fs::create_dir_all(&second).expect("must create second result directory");
    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenFindDialog)
        .expect("find dialog should open");
    app.apply(AppCommand::DialogAccept)
        .expect("find should start");
    let job_id = app.jobs.last_job().expect("find job should exist").id;
    app.handle_background_event(BackgroundEvent::FindEntriesChunk {
        job_id,
        entries: vec![
            FindResultEntry {
                path: root.join("first"),
                is_dir: false,
            },
            FindResultEntry {
                path: second.clone(),
                is_dir: true,
            },
        ],
    });

    let list = find_results_layout(viewport()).list;
    let commands = click_commands(&app, list.x, list.y + 1);
    assert_eq!(commands.primary, AppCommand::FindResultsSelectAt(1));
    assert_eq!(commands.activation, Some(AppCommand::FindResultsOpenEntry));

    app.apply(commands.primary)
        .expect("mouse selection should apply");
    assert!(matches!(
        app.top_route(),
        Route::FindResults(results) if results.cursor == 1
    ));
    app.apply(commands.activation.expect("double click should activate"))
        .expect("find result activation should apply");
    assert_eq!(app.active_panel().cwd, second);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn tree_click_maps_visible_projection_and_offers_open_activation() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-tree-mouse-{stamp}"));
    let alpha = root.join("alpha");
    fs::create_dir_all(&alpha).expect("must create alpha directory");
    fs::create_dir_all(root.join("beta")).expect("must create beta directory");
    let mut app = app_with_loaded_panels(root.clone());
    app.apply(AppCommand::OpenTree).expect("tree should open");
    drain_background(&mut app);

    let list = tree_layout(viewport()).list;
    let commands = click_commands(&app, list.x, list.y + 1);
    assert_eq!(commands.primary, AppCommand::TreeSelectVisibleAt(1));
    assert_eq!(commands.activation, Some(AppCommand::TreeOpenEntry));

    app.apply(commands.primary)
        .expect("tree mouse selection should apply");
    assert!(matches!(
        app.top_route(),
        Route::Tree(tree) if tree.selected_entry().is_some_and(|entry| entry.path == alpha)
    ));
    app.apply(commands.activation.expect("double click should activate"))
        .expect("tree activation should apply");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert_eq!(app.active_panel().cwd, alpha);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn hotlist_click_selects_entry_and_offers_open_activation() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-hotlist-mouse-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.settings_mut().configuration.hotlist = vec![
        HotlistEntry::new("First", root.join("first")),
        HotlistEntry::new("Second", root.join("second")),
    ];
    app.apply(AppCommand::OpenHotlist)
        .expect("hotlist should open");

    let list = hotlist_layout(viewport()).list;
    let commands = click_commands(&app, list.x, list.y + 1);
    assert_eq!(commands.primary, AppCommand::HotlistSelectAt(1));
    assert_eq!(commands.activation, Some(AppCommand::HotlistOpenEntry));

    app.apply(commands.primary)
        .expect("hotlist mouse selection should apply");
    assert_eq!(app.hotlist_cursor, 1);

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn panelize_click_selects_preset_and_offers_accept_activation() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-panelize-mouse-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenPanelizeDialog)
        .expect("panelize dialog should open");

    let list = listbox_dialog_layout(viewport(), 2).list;
    let commands = click_commands(&app, list.x, list.y + 1);
    assert_eq!(commands.primary, AppCommand::DialogListboxSelectAt(1));
    assert_eq!(commands.activation, Some(AppCommand::DialogAccept));

    app.apply(commands.primary)
        .expect("panelize mouse selection should apply");
    assert!(matches!(
        app.top_route(),
        Route::Dialog(dialog)
            if matches!(&dialog.kind, DialogKind::Listbox(listbox) if listbox.selected == 1)
    ));
    app.apply(commands.activation.expect("double click should activate"))
        .expect("panelize activation should apply");
    assert_eq!(app.key_context(), KeyContext::FileManager);
    assert!(app.active_panel().is_panelized());

    fs::remove_dir_all(&root).expect("must remove temp root");
}

#[test]
fn list_hit_testing_rejects_borders_empty_rows_and_tiny_viewports() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    let root = env::temp_dir().join(format!("rc-mouse-bounds-{stamp}"));
    fs::create_dir_all(&root).expect("must create temp root");
    let mut app = AppState::new(root.clone()).expect("app should initialize");
    app.apply(AppCommand::OpenHotlist)
        .expect("empty hotlist should open");
    let list = hotlist_layout(viewport()).list;

    assert_eq!(
        app.commands_for_left_click(list.x, list.y, VIEWPORT_WIDTH, VIEWPORT_HEIGHT),
        None,
        "empty placeholder must not behave like a real entry"
    );
    assert_eq!(app.commands_for_left_click(0, 0, 4, 4), None);

    fs::remove_dir_all(&root).expect("must remove temp root");
}
