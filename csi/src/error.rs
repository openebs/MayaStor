//! Definition of DeviceError used by the attach and detach code.

pub struct DeviceError {
    message: String,
}

impl DeviceError {
    pub fn new(message: &str) -> DeviceError {
        DeviceError {
            message: String::from(message),
        }
    }
}

impl std::fmt::Debug for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for DeviceError {
    fn description(&self) -> &str {
        &self.message
    }
}

impl From<std::io::Error> for DeviceError {
    fn from(error: std::io::Error) -> DeviceError {
        DeviceError {
            message: format!("{}", error),
        }
    }
}

impl From<failure::Error> for DeviceError {
    fn from(error: failure::Error) -> DeviceError {
        DeviceError {
            message: format!("{}", error),
        }
    }
}

impl From<std::num::ParseIntError> for DeviceError {
    fn from(error: std::num::ParseIntError) -> DeviceError {
        DeviceError {
            message: format!("{}", error),
        }
    }
}

impl From<uuid::parser::ParseError> for DeviceError {
    fn from(error: uuid::parser::ParseError) -> DeviceError {
        DeviceError {
            message: format!("{}", error),
        }
    }
}

impl From<String> for DeviceError {
    fn from(message: String) -> DeviceError {
        DeviceError {
            message,
        }
    }
}
