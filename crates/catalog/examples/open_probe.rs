use std::time::Instant;
use lightcraft_catalog::journal::Journal;
use lightcraft_catalog::store::FsStore;
fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let t = Instant::now();
    let store = FsStore::open(std::path::Path::new(&dir)).unwrap();
    let (j, c, r) = Journal::open(Box::new(store)).unwrap();
    eprintln!("Journal::open: {:?} photos {} replayed {}", t.elapsed(), c.len(), r.replayed);
    let out = std::process::Command::new("ps").args(["-o","rss=","-p",&std::process::id().to_string()]).output().unwrap();
    eprintln!("rss after open: {} MB", String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap_or(0)/1024);
    for _ in 0..3 {
        let t = Instant::now();
        let f = lightcraft_catalog::Filter::default();
        let ids = c.query(&f, &lightcraft_catalog::Sort::default());
        let q = t.elapsed();
        let t = Instant::now();
        let (v, n) = c.arrange_stacks_counted(&ids);
        eprintln!("query {:?} ({} ids)  arrange_stacks {:?} ({} shown, {} counted)", q, ids.len(), t.elapsed(), v.len(), n.len());
    }
    let t = Instant::now();
    drop(c);
    drop(j);
    eprintln!("drop: {:?}", t.elapsed());
}
