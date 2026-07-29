// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// cxx-qt generates the QObject subclass from the Rust bridge. The one hand-written C++ file
// is `viewport.cpp`, which exists only because Qt's native-interface accessors return
// platform pointers that no binding generator can usefully wrap.

use std::path::PathBuf;
use std::process::Command;

/// Qt's version-scoped private include directory, e.g. `include/QtGui/6.11.1/QtGui`.
///
/// Needed for `qpa/qplatformnativeinterface.h`, which is how a `wl_surface` is obtained
/// without depending on qtwayland's headers for one pointer. Discovered from qmake rather
/// than hardcoded, because the path carries Qt's patch version and pinning it here would
/// break on every Qt bump.
fn qt_private_include() -> Option<PathBuf> {
    let qmake = std::env::var("QMAKE").unwrap_or_else(|_| "qmake6".into());
    let query = |k: &str| -> Option<String> {
        let out = Command::new(&qmake).arg("-query").arg(k).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let headers = PathBuf::from(query("QT_INSTALL_HEADERS")?);
    let version = query("QT_VERSION")?;
    let dir = headers.join("QtGui").join(&version);
    dir.exists().then_some(dir)
}

fn main() {
    // Declared explicitly. `CxxQtBuilder::cpp_file` did not emit these, so editing
    // `viewport.cpp` left the previous object file in place and `cargo build` reported success
    // in three seconds having compiled nothing. Two rounds of Vulkan diagnosis were run against
    // a stale binary before the missing instrumentation gave it away.
    for f in [
        "src/viewport.cpp",
        "src/viewport.h",
        "src/viewport_platform.h",
        "src/viewport_vulkan.cpp",
        "src/viewport_vulkan.h",
        "src/viewport_mac.mm",
        "src/viewport_mac.h",
        "qml/Main.qml",
    ] {
        println!("cargo::rerun-if-changed={f}");
    }
    let apple = std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple");
    let mut builder = cxx_qt_build::CxxQtBuilder::new_qml_module(
        cxx_qt_build::QmlModule::new("com.strider.editor").qml_file("qml/Main.qml"),
    )
    .qt_module("Gui")
    .qt_module("Quick")
    .file("src/main.rs")
    .cpp_file("src/viewport.cpp")
    .include_dir("src");

    // One platform file, chosen by the target. This is the only place in the build that names a
    // backend, and it mirrors the single `#[cfg_attr(path)]` that chooses `share_*.rs` on the Rust
    // side: `viewport.cpp` contains no `#ifdef` and neither does `main.rs`.
    //
    // The Apple half is Objective-C++ in its own translation unit rather than making
    // `viewport.cpp` an `.mm`, so the portable file stays compilable by a plain C++ compiler. `cc`
    // infers the language from the extension; the frameworks have to be named for the LINKER,
    // which is a `cargo::` directive rather than anything `cc` can do.
    // SAFETY (the API's own requirement): this adds a source file and does not change the ABI of
    // anything cxx generated.
    builder = unsafe {
        builder.cc_builder(move |cc| {
            if apple {
                cc.file("src/viewport_mac.mm").flag("-fobjc-arc");
            } else {
                cc.file("src/viewport_vulkan.cpp");
            }
        })
    };
    if apple {
        println!("cargo::rustc-link-lib=framework=AppKit");
    }

    match qt_private_include() {
        Some(dir) => {
            // `cc_builder`, not `include_dir`: the latter governs where *generated* headers
            // are placed and does not reach the compiler invocation for `cpp_file`.
            //
            // SAFETY (the API's own requirement): this only appends include paths. It does
            // not change the ABI of anything cxx generated.
            builder = unsafe {
                builder.cc_builder(|cc| {
                    // Both levels are needed: the header is reachable as `QtGui/qpa/...`
                    // from one and as `qpa/...` from the other, and Qt's own headers use
                    // both spellings.
                    cc.include(&dir);
                    cc.include(dir.join("QtGui"));
                })
            };
        }
        // Not a warning on Apple, where there is no Wayland and nothing needs them.
        None if apple => {}
        None => println!(
            "cargo::warning=Qt private headers not found; the Wayland surface path will not \
             compile. Set QMAKE to a qmake that reports this installation."
        ),
    }

    // moc, run by hand.
    //
    // `CxxQtBuilder`'s own moc support generates the file and does not compile it, which
    // surfaces as `undefined reference to StriderViewportItem::staticMetaObject` at link time
    // — the metaobject exists in a translation unit nobody built. Rather than work out which
    // arrangement it expects, this runs moc on the header and compiles the result, which is
    // the plain Qt recipe and is fully visible here.
    match moc_header("src/viewport.h") {
        Some(generated) => {
            builder = unsafe {
                builder.cc_builder(move |cc| {
                    cc.file(&generated);
                })
            };
        }
        None => println!("cargo::warning=moc not found; the QML viewport item will not link"),
    }

    builder.build();
}

/// Run moc on a header and return the generated source.
fn moc_header(header: &str) -> Option<PathBuf> {
    let qmake = std::env::var("QMAKE").unwrap_or_else(|_| "qmake6".into());
    let libexec = Command::new(&qmake)
        .args(["-query", "QT_INSTALL_LIBEXECS"])
        .output()
        .ok()
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string()))?;
    let moc = libexec.join("moc");
    if !moc.exists() {
        return None;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").ok()?).join("moc_viewport_manual.cpp");
    let status = Command::new(&moc)
        .arg(header)
        .arg("-o")
        .arg(&out)
        .status()
        .ok()?;
    println!("cargo::rerun-if-changed={header}");
    status.success().then_some(out)
}
