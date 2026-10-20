use std::error::Error as _;

use idevice::IdeviceError;
use idevice::afc::errors::AfcError;
use orchard_core::Error;

/// Maps an `idevice` error into the closest Orchard error, prefixed with `context`.
pub(crate) fn device_error(context: &str, e: IdeviceError) -> Error {
    match &e {
        IdeviceError::DeviceNotFound => Error::NotFound(format!("{context}: device not connected")),
        IdeviceError::NotFound | IdeviceError::Afc(AfcError::ObjectNotFound) => {
            Error::NotFound(context.to_string())
        }
        IdeviceError::Plist(_)
        | IdeviceError::Utf8(_)
        | IdeviceError::Utf8Error
        | IdeviceError::NotEnoughBytes(..)
        | IdeviceError::UnexpectedResponse(_) => {
            Error::Protocol(format!("{context}: {}", describe(&e)))
        }
        _ => Error::Device(format!("{context}: {}", describe(&e))),
    }
}

/// Display of an error followed by its sources, so `io` and TLS details are not lost.
fn describe(e: &IdeviceError) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !out.ends_with(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = s.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_keep_their_detail() {
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let e = device_error("usbmuxd", IdeviceError::Socket(io));
        assert_eq!(e.kind(), "device");
        assert!(e.to_string().contains("refused"), "{e}");
    }

    #[test]
    fn afc_not_found_maps_to_not_found() {
        let e = device_error("/DCIM", IdeviceError::Afc(AfcError::ObjectNotFound));
        assert_eq!(e.kind(), "notFound");
        assert_eq!(
            device_error("x", IdeviceError::UnexpectedResponse("bad".into())).kind(),
            "protocol"
        );
    }
}
