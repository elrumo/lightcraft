//! Background model runs ([`crate::in_background`]): a pool of their own with lowered threads, chosen by the thread
//! that runs the model, and nobody else's threads touched.

use super::BACKGROUND;

/// The calling thread's nice value (field 19 of its `stat`; the name in field 2 may hold spaces).
#[cfg(all(feature = "onnx", target_os = "linux"))]
fn nice() -> i32 {
    let stat = std::fs::read_to_string("/proc/thread-self/stat").unwrap();
    stat.rsplit(')').next().unwrap().split_whitespace().nth(16).unwrap().parse().unwrap()
}

#[test]
fn background_work_is_the_threads_own_and_ends_with_it() {
    assert_eq!(BACKGROUND.get(), None);
    crate::in_background(3, || {
        assert_eq!(BACKGROUND.get(), Some((3, false)));
        crate::in_background(0, || assert_eq!(BACKGROUND.get(), Some((1, false)), "at least one thread"));
        crate::in_background_at(2, true, || assert_eq!(BACKGROUND.get(), Some((2, true)), "the lowest priority asked for"));
        assert_eq!(BACKGROUND.get(), Some((3, false)), "the outer one again");
        // (another thread's models are not this thread's background work)
        assert_eq!(std::thread::spawn(|| BACKGROUND.get()).join().unwrap(), None);
    });
    assert_eq!(BACKGROUND.get(), None);
    let unwound = std::panic::catch_unwind(|| crate::in_background(2, || panic!("a model that panics")));
    assert!(unwound.is_err());
    assert_eq!(BACKGROUND.get(), None, "put back when the work unwinds");
}

#[cfg(all(feature = "onnx", not(target_arch = "wasm32")))]
#[test]
fn background_models_get_lowered_threads_of_their_own() {
    let pool = super::background_pool(2, false);
    let threads = pool.run(|| rayon::broadcast(|_| std::thread::current().name().map(str::to_owned)));
    assert_eq!(threads.len(), 2);
    assert!(threads.iter().all(|n| n.as_deref().is_some_and(|n| n.starts_with("rten-"))), "{threads:?}");
    assert!(std::sync::Arc::ptr_eq(&pool, &super::background_pool(2, false)), "kept while the size stays");
    assert!(!std::sync::Arc::ptr_eq(&pool, &super::background_pool(3, false)), "another size, another pool");
    #[cfg(target_os = "linux")]
    {
        let before = nice();
        assert_eq!(pool.run(|| rayon::broadcast(|_| nice())), [10, 10], "below the default, not the lowest");
        // rayon's own pool (the interactive work's) and this thread keep theirs
        assert!(rayon::broadcast(|_| nice()).iter().all(|n| *n == before));
        assert_eq!(nice(), before);
    }
    // Background work nobody waits for (a desktop indexing its library) asks for the lowest priority: a pool of its
    // own, its threads at the lowest class (Apple's efficiency cores; nice 19 on Linux), the other pools left as they
    // are. (Here, after the checks above: the pool is kept process-wide.)
    let low = super::background_pool(2, true);
    let utility = super::background_pool(2, false);
    assert!(!std::sync::Arc::ptr_eq(&low, &utility), "a priority of its own, a pool of its own");
    #[cfg(target_os = "linux")]
    assert_eq!(low.run(|| rayon::broadcast(|_| nice())), [19, 19]);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        let lowest = std::thread::spawn(|| {
            lightcraft_sysmem::lower_thread_priority();
            lightcraft_sysmem::Qos::current()
        })
        .join()
        .unwrap();
        assert!(low.run(|| rayon::broadcast(|_| lightcraft_sysmem::Qos::current())).iter().all(|q| *q == lowest));
        assert!(utility.run(|| rayon::broadcast(|_| lightcraft_sysmem::Qos::current())).iter().all(|q| *q != lowest));
    }
}
