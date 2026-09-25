//! Key smoke test (Windows / wine, engine serving): type the keysyms given
//! on the command line (letters as they are, {Up} {Down} {Left} {Right}
//! {space}), then wait a few seconds so the candidate window can be seen.
fn main() {
    use ime_proto::*;
    let keys = std::env::args().nth(1).unwrap_or_default();
    let sid = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(1u32);
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(pipe_name(sid)).expect("open pipe");
    let mut call = |r: Request| -> Reply {
        write_frame(&mut f, &r).unwrap();
        read_frame(&mut f).unwrap()
    };
    let Reply::Hello { session, .. } = call(Request::Hello { version: VERSION, pid: 1, exe: "notepad.exe".into(), notify: 0, draws: false }) else { panic!() };
    call(Request::Focus { session, on: true });
    call(Request::Caret { session, x: 100, y: 100, h: 20 });
    let mut it = keys.chars();
    while let Some(c) = it.next() {
        let code = if c == '{' {
            let name: String = it.by_ref().take_while(|c| *c != '}').collect();
            match name.as_str() {
                "Up" => keysym::UP,
                "Down" => keysym::DOWN,
                "Left" => keysym::LEFT,
                "Right" => keysym::RIGHT,
                "space" => 0x20,
                _ => 0,
            }
        } else {
            c as u32
        };
        println!("{c} -> {:?}", call(Request::Key { session, keycode: code, mask: 0 }));
    }
    std::thread::sleep(std::time::Duration::from_secs(6));
    println!("{:?}", call(Request::Bye { session }));
}
