// ── src/hotkey.rs : deep OS integration, rung 2 (cont.) - global summon hotkey ──
//
// A system-wide hotkey (Ctrl+Alt+J) that opens/focuses the Jarvis HUD from
// ANYWHERE, so you can summon Jarvis without hunting for the window. Windows-only.
//
// RegisterHotKey with a null window posts WM_HOTKEY to the *registering thread's*
// message queue, so registration and the GetMessage loop must live on the same
// dedicated thread. Off with JARVIS_HOTKEY=off.

use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, MOD_CONTROL};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

const VK_J: u32 = 0x4A; // virtual-key code for 'J'

pub fn spawn(url: String) {
    if std::env::var("JARVIS_HOTKEY").unwrap_or_default() == "off" {
        return;
    }
    std::thread::spawn(move || unsafe {
        if RegisterHotKey(None, 1, MOD_CONTROL | MOD_ALT, VK_J).is_err() {
            eprintln!("[hotkey] Ctrl+Alt+J is taken by another app; summon hotkey disabled");
            return;
        }
        eprintln!("[hotkey] press Ctrl+Alt+J anywhere to summon Jarvis");
        let mut msg = MSG::default();
        // blocks until a message; WM_HOTKEY fires on the keypress
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_HOTKEY {
                // open/focus the HUD in the default browser
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "start", "", &url])
                    .spawn();
            }
        }
    });
}
