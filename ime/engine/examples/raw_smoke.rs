//! RawKey smoke test (Windows / wine, engine serving): n i h k space as raw
//! virtual keys, then Shift+1 and the keypad dot.
fn main() {
    use ime_proto::*;
    let sid = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1u32);
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(pipe_name(sid)).expect("open pipe");
    let mut call = |r: Request| -> Reply {
        write_frame(&mut f, &r).unwrap();
        read_frame(&mut f).unwrap()
    };
    let Reply::Hello { session, .. } = call(Request::Hello { version: VERSION, pid: 1, exe: "notepad.exe".into(), notify: 0, draws: false, build: String::new() }) else { panic!() };
    let hkl = 0x0409_0409u64;
    for (vk, scan, flags) in [(0x4Eu16, 0x31u16, 0u32), (0x49, 0x17, 0), (0x48, 0x23, 0), (0x4B, 0x25, 0), (0x20, 0x39, 0), (0x31, 0x02, rawkey::SHIFT), (0x6E, 0x53, 0), (0x56, 0x2F, rawkey::CONTROL)] {
        println!("vk {vk:#x} -> {:?}", call(Request::RawKey { session, vk, scan, flags, hkl }));
    }
    println!("{:?}", call(Request::Bye { session }));
}
