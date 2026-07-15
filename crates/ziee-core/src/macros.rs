/// Convert PascalCase to camelCase by lowercasing the first character
/// This is a helper function that will be used at runtime
pub fn pascal_to_camel_case(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let mut chars: Vec<char> = s.chars().collect();
    chars[0] = chars[0].to_lowercase().next().unwrap_or(chars[0]);
    chars.into_iter().collect()
}

/// Macro to define an SSE event enum with automatic implementation of event helpers and Into<Event> trait
///
/// This macro defines the enum and automatically generates:
/// - event_name() method that converts PascalCase variants to camelCase
/// - data() method that serializes the variant data to JSON
/// - Into<axum::response::sse::Event> implementation
///
/// Usage:
/// ```ignore
/// sse_event_enum! {
///     #[derive(Debug, Clone, Serialize, JsonSchema)]
///     #[serde(rename_all = "camelCase")]
///     pub enum SSEMyEvent {
///         Connected(SomeData),
///         Update(OtherData),
///         LogUpdate(String),
///         CreatedBranch(BranchData),
///     }
/// }
/// ```
#[macro_export]
macro_rules! sse_event_enum {
    (
        $(#[$attr:meta])*
        $vis:vis enum $enum_name:ident {
            $($variant:ident($data_type:ty)),+ $(,)?
        }
    ) => {
        $(#[$attr])*
        #[serde(rename_all = "camelCase")]
        $vis enum $enum_name {
            $($variant($data_type),)+
        }

        impl $enum_name {
            pub fn event_name(&self) -> &'static str {
                match self {
                    $(
                        Self::$variant(_) => {
                            // Use a static cache to avoid repeated string operations
                            static EVENT_NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
                            EVENT_NAME.get_or_init(|| {
                                $crate::macros::pascal_to_camel_case(stringify!($variant))
                            })
                        },
                    )+
                }
            }

            pub fn data(&self) -> Result<String, serde_json::Error> {
                match self {
                    $(
                        Self::$variant(data) => serde_json::to_string(data),
                    )+
                }
            }
        }

        impl Into<axum::response::sse::Event> for $enum_name {
            fn into(self) -> axum::response::sse::Event {
                axum::response::sse::Event::default()
                    .event(self.event_name())
                    .data(self.data().unwrap_or_default())
            }
        }
    };
}

/// Implement From<String> for enums that have from_str() method
/// Usage: impl_string_to_enum!(EngineType);
/// This allows SQLx to automatically convert database strings to enum types
#[macro_export]
macro_rules! impl_string_to_enum {
    ($enum_type:ty) => {
        impl From<String> for $enum_type {
            fn from(s: String) -> Self {
                Self::from_str(&s).unwrap_or_else(|| {
                    panic!(
                        "Invalid enum value '{}' for type {}",
                        s,
                        std::any::type_name::<$enum_type>()
                    )
                })
            }
        }

        impl From<&str> for $enum_type {
            fn from(s: &str) -> Self {
                Self::from_str(s).unwrap_or_else(|| {
                    panic!(
                        "Invalid enum value '{}' for type {}",
                        s,
                        std::any::type_name::<$enum_type>()
                    )
                })
            }
        }
    };
}

/// Implement From<serde_json::Value> for types that implement Default and DeserializeOwned
/// Usage: impl_json_from!(MyStruct);
/// This allows automatic JSON value conversion with fallback to default
#[macro_export]
macro_rules! impl_json_from {
    ($struct_type:ty) => {
        impl From<serde_json::Value> for $struct_type {
            fn from(value: serde_json::Value) -> Self {
                serde_json::from_value(value).unwrap_or_default()
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::pascal_to_camel_case;

    #[test]
    fn pascal_to_camel_case_lowercases_first_char_only() {
        assert_eq!(pascal_to_camel_case("Connected"), "connected");
        assert_eq!(pascal_to_camel_case("LogUpdate"), "logUpdate");
        assert_eq!(pascal_to_camel_case("CreatedBranch"), "createdBranch");
        // Single-char + already-camel inputs.
        assert_eq!(pascal_to_camel_case("A"), "a");
        assert_eq!(pascal_to_camel_case("alreadyCamel"), "alreadyCamel");
        // Empty string is a valid no-op (guards the `chars[0]` index).
        assert_eq!(pascal_to_camel_case(""), "");
        // Non-ASCII leading char must not panic and must lowercase.
        assert_eq!(pascal_to_camel_case("Éclair"), "éclair");
    }

    // --- sse_event_enum! macro ---
    //
    // The macro is the single source of the hardware/download/etc SSE wire
    // format: it must generate `event_name()` (PascalCase variant → camelCase),
    // `data()` (serialize the variant payload to JSON), and `Into<sse::Event>`.

    #[derive(Debug, Clone, serde::Serialize)]
    struct Payload {
        value: u32,
    }

    crate::sse_event_enum! {
        #[derive(Debug, Clone, serde::Serialize)]
        pub enum TestEvent {
            Connected(String),
            LogUpdate(Payload),
        }
    }

    #[test]
    fn sse_event_enum_event_name_is_camel_case() {
        let a = TestEvent::Connected("hi".to_string());
        let b = TestEvent::LogUpdate(Payload { value: 7 });
        assert_eq!(a.event_name(), "connected");
        assert_eq!(b.event_name(), "logUpdate");
    }

    #[test]
    fn sse_event_enum_data_serializes_variant_payload() {
        let a = TestEvent::Connected("hello".to_string());
        assert_eq!(a.data().unwrap(), "\"hello\"");
        let b = TestEvent::LogUpdate(Payload { value: 42 });
        let v: serde_json::Value = serde_json::from_str(&b.data().unwrap()).unwrap();
        assert_eq!(v["value"], 42);
    }

    #[test]
    fn sse_event_enum_into_axum_event() {
        // Into<axum::response::sse::Event> must be generated and not panic.
        let _ev: axum::response::sse::Event = TestEvent::Connected("x".to_string()).into();
    }

    // --- impl_string_to_enum! macro ---

    #[derive(Debug, PartialEq)]
    enum Color {
        Red,
        Blue,
    }

    impl Color {
        fn from_str(s: &str) -> Option<Self> {
            match s {
                "red" => Some(Color::Red),
                "blue" => Some(Color::Blue),
                _ => None,
            }
        }
    }

    crate::impl_string_to_enum!(Color);

    #[test]
    fn impl_string_to_enum_from_str_and_ref() {
        assert_eq!(Color::from("red".to_string()), Color::Red);
        assert_eq!(Color::from("blue"), Color::Blue);
    }

    #[test]
    #[should_panic(expected = "Invalid enum value")]
    fn impl_string_to_enum_panics_on_unknown() {
        let _ = Color::from("green".to_string());
    }

    // --- impl_json_from! macro ---

    #[derive(Debug, Default, PartialEq, serde::Deserialize)]
    struct Settings {
        #[serde(default)]
        enabled: bool,
        #[serde(default)]
        count: u32,
    }

    crate::impl_json_from!(Settings);

    #[test]
    fn impl_json_from_parses_value() {
        let s: Settings = serde_json::json!({ "enabled": true, "count": 3 }).into();
        assert_eq!(s, Settings { enabled: true, count: 3 });
    }

    #[test]
    fn impl_json_from_falls_back_to_default_on_bad_value() {
        // A shape that fails to deserialize falls back to Default (no panic).
        let s: Settings = serde_json::json!("not an object").into();
        assert_eq!(s, Settings::default());
    }
}
