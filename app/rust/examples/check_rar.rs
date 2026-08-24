fn main() {
    let path = std::env::args().nth(1).unwrap();
    match unrar::Archive::new(&path).open_for_processing() {
        Ok(mut archive) => {
            let mut count = 0;
            loop {
                match archive.read_header() {
                    Ok(Some(header)) => {
                        let name = header.entry().filename.to_string_lossy().to_string();
                        println!("entry: {name}");
                        archive = match header.read() {
                            Ok((bytes, next)) => {
                                println!("  read {} bytes", bytes.len());
                                count += 1;
                                next
                            }
                            Err(e) => {
                                println!("  read error: {e}");
                                break;
                            }
                        };
                    }
                    Ok(None) => break,
                    Err(e) => {
                        println!("header error: {e}");
                        break;
                    }
                }
            }
            println!("entries read: {count}");
        }
        Err(e) => println!("open error: {e}"),
    }
}
