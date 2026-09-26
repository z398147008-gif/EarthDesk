//! Smoke test for the engine's pipe (run on Windows / wine while
//! EarthDeskIME.exe serves): Status, Hello, a few keys, Caret, Bye.
//!   cargo run --example pipe_smoke -- <session id>
fn main() {
    use ime_proto::*;
    let sid = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1u32);
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(pipe_name(sid)).expect("open pipe");
    let mut call = |r: Request| -> Reply {
        write_frame(&mut f, &r).unwrap();
        read_frame(&mut f).unwrap()
    };
    println!("{:?}", call(Request::Status));
    let Reply::Hello { session, .. } = call(Request::Hello { version: VERSION, pid: 1, exe: "notepad.exe".into(), notify: 0, draws: false, build: String::new() }) else { panic!("hello") };
    for c in "wojntm".chars() {
        println!("{c:?} {:?}", call(Request::Key { session, keycode: c as u32, mask: 0 }));
    }
    println!("{:?}", call(Request::Caret { session, x: 100, y: 100, h: 20 }));
    // Leave the candidate window up for a moment (to look at it).
    std::thread::sleep(std::time::Duration::from_secs(3));
    println!("{:?}", call(Request::Bye { session }));
}
