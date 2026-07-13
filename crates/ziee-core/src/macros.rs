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
