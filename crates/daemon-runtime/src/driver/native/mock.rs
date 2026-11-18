//! Mock native driver wrapper.

define_native_driver! {
    /// Native driver for the in-process mock agent used by tests.
    ///
    /// Driver id: `mock`.
    MockNativeDriver => "mock"
}
