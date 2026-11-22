//! `--role worker`: the engine on stdin/stdout frames. Started by the launcher inside the worker sandbox.

use margin_engine::EngineConfig;

use crate::args::Args;

pub fn run(args: &Args) -> Result<(), String> {
    let root = args.folder.clone().ok_or("worker: --root is required")?;
    let (requests, out) = margin_proto::link::server_over_io(std::io::stdin(), std::io::stdout());
    margin_engine::run(EngineConfig { root, cache_dir: args.dir.clone(), opts: args.opts.clone(), threads: 0 }, requests, out);
    Ok(())
}
