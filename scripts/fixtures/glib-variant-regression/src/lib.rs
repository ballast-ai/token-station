// 官方 glib 0.18.5 variant_iter.rs 的三个字符串迭代器用例，函数体保持原样。
// 在真正依赖的 glib 上运行，不能用本地复制实现替代优化 FFI 回归。
#[cfg(test)]
mod tests {
    use glib::{prelude::*, Variant};
    #[test]
    fn test_variant_str_iter_nth() {
        let v = Variant::array_from_iter::<String>([
            "0".to_string().to_variant(),
            "1".to_string().to_variant(),
            "2".to_string().to_variant(),
            "3".to_string().to_variant(),
            "4".to_string().to_variant(),
            "5".to_string().to_variant(),
        ]);

        let mut iter = v.array_iter_str().unwrap();

        assert_eq!(iter.len(), 6);
        assert_eq!(iter.nth(1), Some("1"));
        assert_eq!(iter.len(), 4);
        assert_eq!(iter.next(), Some("2"));
        assert_eq!(iter.nth_back(2), Some("3"));
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
    }

    #[test]
    fn test_variant_str_iter_count() {
        let v = Variant::array_from_iter::<String>([
            "0".to_string().to_variant(),
            "1".to_string().to_variant(),
            "2".to_string().to_variant(),
        ]);

        let iter = v.array_iter_str().unwrap();

        assert_eq!(iter.len(), 3);
        assert_eq!(iter.count(), 3);
    }

    #[test]
    fn test_variant_str_iter_last() {
        let v = Variant::array_from_iter::<String>([
            "0".to_string().to_variant(),
            "1".to_string().to_variant(),
            "2".to_string().to_variant(),
        ]);

        let iter = v.array_iter_str().unwrap();

        assert_eq!(iter.len(), 3);
        assert_eq!(iter.last(), Some("2"));
    }
    #[test]
    fn nonempty_next_back_preserves_order() {
        let value = ["first", "middle", "last"].to_variant();
        let mut iter = value.array_iter_str().unwrap();
        assert_eq!(iter.next_back(), Some("last"));
        assert_eq!(iter.next(), Some("first"));
        assert_eq!(iter.next_back(), Some("middle"));
        assert_eq!(iter.next_back(), None);
    }
}
