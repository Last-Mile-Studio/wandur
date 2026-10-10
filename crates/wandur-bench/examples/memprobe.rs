//! Terminal memory: `cargo run --release -p wandur-bench --example memprobe`.
//!
//! 1. Live heap of one terminal as it fills.
//! 2. Live heap and resident memory of 16 empty terminals (is vte's 2 MB reservation resident?).
//! 3. Retained per terminal by scrollback size, 120 columns, history full.
//! 4. Released at the bound: 20,000 lines with combining marks and colours through a 2,000 row
//!    history, then the scrollback lowered to 500.
use wandur_app::sysstat::{self, CountingAllocator};
use wandur_term::{TermSize, Terminal};
#[global_allocator]
static A: CountingAllocator = CountingAllocator;

fn kb(b: u64) -> u64 {
    b / 1024
}

fn fill(t: &mut Terminal, lines: u64, fancy: bool) {
    let start = t.lines_total();
    while t.lines_total() < start + lines {
        if fancy {
            t.feed(
                "\x1b[1;31mThe orc\x1b[0m hits you with a cafe\u{301} \x1b[38;2;200;10;90mclub\x1b[0m.\n".as_bytes(),
            );
        } else {
            t.feed(b"\x1b[1;31mThe orc\x1b[0m hits you with \x1b[38;5;200mclub\x1b[0m.\r\n");
        }
    }
}

fn main() {
    println!("1. one terminal, 120x50, 2,000 rows of scrollback");
    let l0 = sysstat::live_heap_bytes();
    let mut t = Terminal::new(TermSize::new(120, 50), 2_000);
    println!("   new: {} KB", kb(sysstat::live_heap_bytes() - l0));
    for n in [100u64, 1000, 2000, 5000] {
        let more = n - t.lines_total();
        fill(&mut t, more, false);
        println!(
            "   {n} lines: {} KB, history {}",
            kb(sysstat::live_heap_bytes() - l0),
            t.history_len()
        );
    }
    drop(t);

    println!("2. sixteen empty terminals");
    let (h0, r0) = (sysstat::live_heap_bytes(), sysstat::resident_bytes());
    let many: Vec<Terminal> = (0..16).map(|_| Terminal::new(TermSize::new(120, 50), 2_000)).collect();
    let (h1, r1) = (sysstat::live_heap_bytes(), sysstat::resident_bytes());
    println!(
        "   live heap +{} KB ({} KB each), resident +{} KB ({} KB each)",
        kb(h1 - h0),
        kb(h1 - h0) / 16,
        kb(r1.saturating_sub(r0)),
        kb(r1.saturating_sub(r0)) / 16
    );
    drop(many);

    println!("3. retained with the history full, 120 columns");
    for rows in [500usize, 1_000, 2_000, 5_000, 10_000] {
        let (h0, r0) = (sysstat::live_heap_bytes(), sysstat::resident_bytes());
        let mut t = Terminal::new(TermSize::new(120, 50), rows);
        fill(&mut t, rows as u64 + 100, false);
        let (h1, r1) = (sysstat::live_heap_bytes(), sysstat::resident_bytes());
        println!(
            "   {rows:>6} rows: live heap {} KB, resident +{} KB",
            kb(h1 - h0),
            kb(r1.saturating_sub(r0))
        );
        drop(t);
    }

    println!("4. released at the bound");
    let h0 = sysstat::live_heap_bytes();
    let mut t = Terminal::new(TermSize::new(120, 50), 2_000);
    fill(&mut t, 2_100, true);
    let full = sysstat::live_heap_bytes() - h0;
    fill(&mut t, 20_000, true);
    let after = sysstat::live_heap_bytes() - h0;
    println!(
        "   full: {} KB; after 20,000 more lines with combining marks: {} KB (history {})",
        kb(full),
        kb(after),
        t.history_len()
    );
    t.set_scrollback(500);
    fill(&mut t, 10, true);
    println!(
        "   scrollback lowered to 500: {} KB (history {})",
        kb(sysstat::live_heap_bytes() - h0),
        t.history_len()
    );
}
