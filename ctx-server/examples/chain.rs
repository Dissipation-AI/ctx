fn main() {
    let me = std::process::id() as i32;
    for p in ctx_server::sys::ancestors(me) {
        println!(
            "{p:?} args={:?}",
            ctx_server::sys::proc_args(p.pid).map(|a| a.into_iter().take(2).collect::<Vec<_>>())
        );
    }
}
