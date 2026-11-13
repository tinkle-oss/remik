#![no_main]

use libfuzzer_sys::fuzz_target;
use remik::protocol::{Limits, decode};

fuzz_target!(|input: &[u8]| {
    for limits in [
        Limits::default(),
        Limits {
            max_frame_bytes: 4096,
            max_bulk_bytes: 1024,
            max_args: 16,
        },
    ] {
        if let Ok(Some((args, used))) = decode(input, limits) {
            assert!(used > 0 && used <= input.len() && used <= limits.max_frame_bytes);
            assert!(!args.is_empty() && args.len() <= limits.max_args);
            assert!(args.iter().all(|arg| arg.len() <= limits.max_bulk_bytes));
            assert!(args.iter().map(|arg| arg.len()).sum::<usize>() <= used);
            assert_eq!(decode(&input[..used], limits), Ok(Some((args, used))));
        }
    }
});
