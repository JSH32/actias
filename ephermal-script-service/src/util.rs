macro_rules! safe_divide {
    ($a:expr, $b:expr) => {
        if $a == 0 {
            0
        } else {
            $a / $b
        }
    };
}

pub(crate) use safe_divide;
