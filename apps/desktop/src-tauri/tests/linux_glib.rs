#![cfg(target_os = "linux")]

use glib::{prelude::*, Variant};
use std::hint::black_box;

fn variant(values: &[&str]) -> Variant {
    black_box(Variant::array_from_iter::<String>(
        values.iter().map(|value| value.to_variant()),
    ))
}

#[test]
fn variant_string_iteration_preserves_empty_and_utf8_values() {
    let values = [
        "",
        "ASCII",
        "Rust",
        "GLib",
        "界",
        "café",
        "quoted \"text\"",
        "tail",
    ];
    for _ in 0..2_000 {
        let value = variant(black_box(&values));
        let forward: Vec<_> = value.array_iter_str().unwrap().collect();
        assert_eq!(forward, values);
        let reverse: Vec<_> = value.array_iter_str().unwrap().rev().collect();
        assert_eq!(reverse, values.into_iter().rev().collect::<Vec<_>>());
        assert_eq!(
            value.array_iter_str().unwrap().last(),
            values.last().copied()
        );
    }
    let empty = variant(&[]);
    assert_eq!(empty.array_iter_str().unwrap().next(), None);
    assert_eq!(empty.array_iter_str().unwrap().next_back(), None);
}

#[test]
fn variant_string_iteration_supports_mixed_directions() {
    let value = variant(&["0", "1", "2", "3", "4", "5", "6", "7"]);
    let mut iter = value.array_iter_str().unwrap();
    assert_eq!(iter.len(), 8);
    assert_eq!(iter.next(), Some("0"));
    assert_eq!(iter.next_back(), Some("7"));
    assert_eq!(iter.nth(1), Some("2"));
    assert_eq!(iter.next_back(), Some("6"));
    assert_eq!(iter.nth_back(1), Some("4"));
    assert_eq!(iter.len(), 1);
    assert_eq!(iter.next(), Some("3"));
    assert_eq!(iter.next(), None);
    assert_eq!(iter.next_back(), None);
    assert_eq!(iter.size_hint(), (0, Some(0)));
}

#[test]
fn variant_string_iteration_exhausts_without_overflow() {
    let value = variant(&["first", "last"]);
    let mut forward = value.array_iter_str().unwrap();
    assert_eq!(forward.nth(usize::MAX), None);
    assert_eq!(forward.next_back(), None);
    let mut reverse = value.array_iter_str().unwrap();
    assert_eq!(reverse.nth_back(usize::MAX), None);
    assert_eq!(reverse.next(), None);
}
