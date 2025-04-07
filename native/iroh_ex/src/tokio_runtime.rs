use tokio::runtime::Runtime;

use once_cell::sync::Lazy;

pub static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    let cores = num_cpus::get();

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cores)
        .enable_all()
        .build()
        .unwrap()
});
