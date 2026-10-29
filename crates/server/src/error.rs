//! Stable error codes, so that the dashboard and monitoring can react to a failure without
//! parsing its message.

use std::fmt::Display;

use api::ErrorReport;

pub trait ErrorCode: Display {
    /// Stable snake_case identifier; never change one that has been released.
    fn code(&self) -> &'static str;

    fn report(&self) -> ErrorReport {
        ErrorReport {
            code: self.code().to_owned(),
            message: self.to_string(),
        }
    }
}
