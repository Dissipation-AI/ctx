fn main() {
    for a in std::env::args().skip(1) {
        let pid: i32 = a.parse().unwrap();
        let r = ctx_server::sys::responsible_pid(pid);
        let name = r.and_then(ctx_server::sys::proc_info).map(|p| p.name);
        println!("pid {pid} → responsible {r:?} ({name:?})");
    }
}
