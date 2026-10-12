#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod window_capture;

use nocfree_companion::{diagnostics, ui};

use gpui_kit::{
    App, AppContext, Bounds, Focusable, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, actions, px, size,
};

actions!(
    nocfree_companion,
    [
        Quit,
        About,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        ShowLogs,
        ExportLogs,
        Help,
        ReportIssue
    ]
);

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if !arguments.is_empty() {
        match arguments.as_slice() {
            [flag, directory] if flag == "--check-firmware" => {
                match nocfree_companion::release::FirmwareRelease::load_from(std::path::Path::new(
                    directory,
                )) {
                    Ok(release) => println!("Verified firmware package: {}", release.id()),
                    Err(error) => {
                        eprintln!("Firmware package: {error}");
                        std::process::exit(1);
                    }
                }
            }
            [flag] if flag == "--help" || flag == "-h" => {
                println!(
                    "NocFree RMK Companion\nUsage: nocfree-companion [--check-firmware DIRECTORY | --help | --version]"
                );
            }
            [flag] if flag == "--version" || flag == "-v" => {
                println!("NocFree RMK Companion {}", env!("CARGO_PKG_VERSION"));
            }
            _ => {
                eprintln!("Unknown arguments. Use --help.");
                std::process::exit(2);
            }
        }
        return;
    }
    let _ = diagnostics::initialize();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            configure_menus(cx);
            cx.on_app_quit(|_| async {
                diagnostics::shutdown();
            })
            .detach();
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(960.), px(660.)),
                        cx,
                    ))),
                    titlebar: Some(TitlebarOptions {
                        title: Some(
                            if cfg!(feature = "firmware-trial") {
                                "NocFree RMK Companion Trial"
                            } else {
                                "NocFree RMK Companion"
                            }
                            .into(),
                        ),
                        ..Default::default()
                    }),
                    window_min_size: Some(size(px(800.), px(560.))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    set_window_max_size(window);
                    let view = cx.new(ui::Companion::new);
                    window.focus(&view.focus_handle(cx), cx);
                    view
                },
            )
            .expect("could not open NocFree RMK Companion window");
            cx.activate(true);
        });
}

fn configure_menus(cx: &mut App) {
    use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
    use gpui_kit::{OsAction, SystemMenuType};
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Minimize, cx| {
        cx.defer(|cx| {
            if let Some(handle) = cx
                .active_window()
                .or_else(|| cx.windows().into_iter().next())
            {
                let _ = handle.update(cx, |_, window, _| window.minimize_window());
            }
        })
    });
    cx.on_action(|_: &Zoom, cx| {
        cx.defer(|cx| {
            if let Some(handle) = cx
                .active_window()
                .or_else(|| cx.windows().into_iter().next())
            {
                let _ = handle.update(cx, |_, window, _| window.zoom_window());
            }
        })
    });
    cx.on_action(|_: &About, cx| {
        message(
            cx,
            "NocFree RMK Companion",
            &format!(
                "Version {}\nInstall RMK, restore your saved firmware, and check your keyboard.",
                env!("CARGO_PKG_VERSION")
            ),
        );
    });
    cx.on_action(|_: &Help, cx| {
        cx.open_url("https://github.com/campfireshuffle/nocfree-and-rmk/blob/main/README.md")
    });
    cx.on_action(|_: &ReportIssue, cx| {
        cx.open_url("https://github.com/campfireshuffle/nocfree-and-rmk/issues/new")
    });
    cx.on_action(|_: &ShowLogs, cx| {
        if let Err(error) = diagnostics::open_logs() {
            message(cx, "Couldn’t open logs", &error);
        }
    });
    cx.on_action(|_: &ExportLogs, cx| {
        // Menu dispatch may already be updating the window. Capture on the UI
        // thread after that update, before moving PNG bytes to the background.
        cx.defer(|cx| {
            let screenshot = cx
                .active_window()
                .or_else(|| cx.windows().into_iter().next())
                .ok_or_else(|| "Window unavailable.".to_owned())
                .and_then(|handle| {
                    handle
                        .update(cx, |_, window, _| window_capture::capture(window))
                        .map_err(|_| "Window unavailable.".to_owned())?
                });
            cx.spawn(async move |cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { diagnostics::export_logs(screenshot) })
                    .await;
                cx.update(|cx| match result {
                    Ok(path) => {
                        let result = diagnostics::reveal_archive(&path);
                        if result.is_err() {
                            message(
                                cx,
                                "Logs exported",
                                &format!("Your diagnostic ZIP was saved to:\n{}", path.display()),
                            );
                        }
                    }
                    Err(error) => message(cx, "Couldn’t export logs", &error),
                });
            })
            .detach();
        });
    });
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ]);
    cx.set_menus([
        Menu::new("NocFree RMK Companion").items([
            MenuItem::action("About NocFree RMK Companion", About),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide NocFree RMK Companion", Hide),
            MenuItem::action("Hide others", HideOthers),
            MenuItem::action("Show all", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit NocFree RMK Companion", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select all", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
        ]),
        Menu::new("Help").items([
            MenuItem::action("NocFree RMK Companion help", Help),
            MenuItem::separator(),
            MenuItem::action("Show logs", ShowLogs),
            MenuItem::action("Export diagnostic logs…", ExportLogs),
            MenuItem::separator(),
            MenuItem::action("Report an issue…", ReportIssue),
        ]),
    ]);
}

fn message(cx: &mut App, title: &str, detail: &str) {
    let title = title.to_owned();
    let detail = detail.to_owned();
    // Menu dispatch may already be updating the window. Prompt after that update.
    cx.defer(move |cx| {
        if let Some(handle) = cx
            .active_window()
            .or_else(|| cx.windows().into_iter().next())
        {
            let _ = handle.update(cx, |_, window, cx| {
                let answer = window.prompt(
                    gpui_kit::PromptLevel::Info,
                    &title,
                    Some(&detail),
                    &["OK"],
                    cx,
                );
                cx.spawn(async move |_| {
                    let _ = answer.await;
                })
                .detach();
            });
        }
    });
}

#[cfg(target_os = "macos")]
fn set_window_max_size(window: &gpui_kit::Window) {
    use objc2_app_kit::NSView;
    use objc2_foundation::NSSize;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = HasWindowHandle::window_handle(window).expect("native window handle");
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        unreachable!("macOS window must expose an AppKit view");
    };
    // GPUI owns this NSView; its handle stays valid for this call on the UI thread.
    // AppKit enforces the content limit during interactive resize and zoom.
    unsafe {
        let view = &*handle.ns_view.as_ptr().cast::<NSView>();
        view.window()
            .expect("GPUI view must be attached to its window")
            .setContentMaxSize(NSSize::new(1200., 800.));
    }
}

#[cfg(not(target_os = "macos"))]
fn set_window_max_size(_window: &gpui_kit::Window) {}
