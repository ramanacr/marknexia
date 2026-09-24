use std::io::Cursor;

use marknexia_files::bounded_read::{BoundedReadError, BoundedReader};

#[test]
fn exact_limit_is_accepted_and_one_byte_over_is_rejected() {
    let reader = BoundedReader::new(4);
    assert_eq!(reader.read_from(Cursor::new(b"abcd")).unwrap(), b"abcd");
    assert_eq!(
        reader.read_from(Cursor::new(b"abcde")),
        Err(BoundedReadError::TooLarge)
    );
}

#[test]
fn zero_limit_rejects_nonempty_input() {
    let reader = BoundedReader::new(0);
    assert_eq!(reader.read_from(Cursor::new(b"")).unwrap(), b"");
    assert_eq!(
        reader.read_from(Cursor::new(b"x")),
        Err(BoundedReadError::TooLarge)
    );
}
