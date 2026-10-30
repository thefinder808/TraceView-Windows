//! Shows what a log writer can still do while a viewer has its file mapped.
//! A log viewer must not break the app that's writing the log.

use std::fs::{self, File, OpenOptions};
use std::io::Write;

fn describe(result: std::io::Result<()>) -> String {
    match result {
        Ok(()) => "allowed".into(),
        Err(error) => format!("BLOCKED ({error})"),
    }
}

fn main() -> std::io::Result<()> {
    let dir = std::env::temp_dir().join(format!("tv-map-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let path = dir.join("app.log");
    fs::write(&path, vec![b'x'; 1024 * 1024])?;

    let viewer = File::open(&path)?;
    let map = unsafe { memmap2::Mmap::map(&viewer)? };
    drop(viewer);
    println!("viewer has the file mapped ({} bytes)\n", map.len());

    let append = OpenOptions::new()
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(b"new line\n"));
    println!("writer appends:          {}", describe(append));

    let truncate = OpenOptions::new()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_len(0));
    println!("writer truncates:        {}", describe(truncate));

    let shrink = OpenOptions::new()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_len(1024));
    println!("writer shrinks:          {}", describe(shrink));

    let rotated = dir.join("app.log.1");
    let rename = fs::rename(&path, &rotated);
    let rename_worked = rename.is_ok();
    println!("writer renames (rotate): {}", describe(rename));

    let current = if rename_worked { &rotated } else { &path };
    println!("writer deletes:          {}", describe(fs::remove_file(current)));
    println!(
        "name still present after delete: {}",
        current.exists()
    );

    drop(map);
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}
