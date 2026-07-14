/// Build guest function arguments from serde values and asynchronous streams.
///
/// The macro returns `Result<[Arg; N], value::Error>`, where `N` is the number
/// of arguments. Regular values are encoded with
/// [`Value::from_serde`](crate::value::Value::from_serde). Four forms are
/// accepted:
///
/// - `value` for a positional value
/// - `name = value` for a named value
/// - `@stream(stream)` for a positional stream of
///   [`Value`](crate::value::Value)
/// - `name = @stream(stream)` for a named stream
///
/// # Examples
///
/// ```
/// use futures::stream;
/// use isola::{
///     sandbox::{Arg, args},
///     value::Value,
/// };
///
/// let values = stream::iter([Value::from_serde(&1_i64)?]);
/// let args = args![40_i64, increment = 2_i64, values = @stream(values)]?;
///
/// assert!(matches!(args[0], Arg::Positional(_)));
/// assert!(matches!(args[1], Arg::Named(ref name, _) if name == "increment"));
/// assert!(matches!(args[2], Arg::NamedStream(ref name, _) if name == "values"));
/// # Ok::<(), isola::value::Error>(())
/// ```
///
/// # Errors
///
/// Returns [`value::Error`](crate::value::Error) if a non-stream argument
/// cannot be serialized to CBOR.
#[macro_export]
macro_rules! args {
    () => {
        ::core::result::Result::<[$crate::sandbox::Arg; 0], $crate::value::Error>::Ok([])
    };
    ($($tokens:tt)+) => {{
        (|| -> ::core::result::Result<_, $crate::value::Error> {
            Ok($crate::__isola_args_internal!(@array [] $($tokens)+))
        })()
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __isola_args_internal {
    (@array [$($out:expr,)*]) => {
        [$($out,)*]
    };
    (@array [$($out:expr,)*] , $($rest:tt)*) => {
        $crate::__isola_args_internal!(@array [$($out,)*] $($rest)*)
    };
    (@array [$($out:expr,)*] @stream($stream:expr) $(, $($rest:tt)*)?) => {
        $crate::__isola_args_internal!(
            @array
            [
                $($out,)*
                $crate::sandbox::Arg::PositionalStream(::std::boxed::Box::pin($stream)),
            ]
            $($($rest)*)?
        )
    };
    (@array [$($out:expr,)*] $name:ident = @stream($stream:expr) $(, $($rest:tt)*)?) => {
        $crate::__isola_args_internal!(
            @array
            [
                $($out,)*
                $crate::sandbox::Arg::NamedStream(
                    ::std::string::String::from(stringify!($name)),
                    ::std::boxed::Box::pin($stream),
                ),
            ]
            $($($rest)*)?
        )
    };
    (@array [$($out:expr,)*] $name:ident = $value:expr $(, $($rest:tt)*)?) => {
        $crate::__isola_args_internal!(
            @array
            [
                $($out,)*
                $crate::sandbox::Arg::Named(
                    ::std::string::String::from(stringify!($name)),
                    $crate::value::Value::from_serde(&$value)?,
                ),
            ]
            $($($rest)*)?
        )
    };
    (@array [$($out:expr,)*] $value:expr $(, $($rest:tt)*)?) => {
        $crate::__isola_args_internal!(
            @array
            [
                $($out,)*
                $crate::sandbox::Arg::Positional($crate::value::Value::from_serde(&$value)?),
            ]
            $($($rest)*)?
        )
    };
}

#[cfg(test)]
mod tests {
    use futures::stream;

    use crate::{sandbox::Arg, value::Value};

    #[test]
    fn args_macro_supports_positional_and_named_values() {
        let args = crate::sandbox::args![1_i64, x = 3_i64, y = "ok"].expect("args");
        assert_eq!(args.len(), 3);

        match &args[0] {
            Arg::Positional(value) => {
                assert_eq!(value.to_serde::<i64>().expect("i64"), 1);
            }
            _ => panic!("unexpected variant"),
        }
        match &args[1] {
            Arg::Named(name, value) => {
                assert_eq!(name, "x");
                assert_eq!(value.to_serde::<i64>().expect("i64"), 3);
            }
            _ => panic!("unexpected variant"),
        }
        match &args[2] {
            Arg::Named(name, value) => {
                assert_eq!(name, "y");
                assert_eq!(value.to_serde::<String>().expect("string"), "ok");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn args_macro_supports_stream_markers() {
        let stream_arg = stream::empty::<Value>();
        let named_stream = stream::empty::<Value>();
        let args = crate::sandbox::args![@stream(stream_arg), x = @stream(named_stream), y = 2_i64]
            .expect("args");
        assert_eq!(args.len(), 3);
        assert!(matches!(args[0], Arg::PositionalStream(_)));
        assert!(matches!(args[1], Arg::NamedStream(_, _)));
        match &args[2] {
            Arg::Named(name, value) => {
                assert_eq!(name, "y");
                assert_eq!(value.to_serde::<i64>().expect("i64"), 2);
            }
            _ => panic!("unexpected variant"),
        }
    }
}
