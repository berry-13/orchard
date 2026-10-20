use idevice::diagnostics_relay::DiagnosticsRelayClient;
use idevice::screenshotr::ScreenshotService;
use orchard_core::{Error, Result};

use crate::connect::DeviceConnection;
use crate::error::device_error;

impl DeviceConnection {
    /// Captures the screen through screenshotr. Current iOS versions return PNG.
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        // A Wi-Fi pairing tunnel does not list screenshotr.
        if self.is_tunnel() {
            return Err(Error::Device(
                "Screenshots need a USB connection; this device is connected over a Wi-Fi pairing"
                    .into(),
            ));
        }
        let mut service = self.service::<ScreenshotService>().await?;
        service
            .take_screenshot()
            .await
            .map_err(|e| device_error("screenshot", e))
    }

    pub async fn restart(&self) -> Result<()> {
        let mut relay = self.service::<DiagnosticsRelayClient>().await?;
        relay
            .restart()
            .await
            .map_err(|e| device_error("restart", e))
    }

    pub async fn shutdown(&self) -> Result<()> {
        let mut relay = self.service::<DiagnosticsRelayClient>().await?;
        relay
            .shutdown()
            .await
            .map_err(|e| device_error("shutdown", e))
    }

    /// Puts the device to sleep (screen off).
    pub async fn sleep(&self) -> Result<()> {
        let mut relay = self.service::<DiagnosticsRelayClient>().await?;
        relay.sleep().await.map_err(|e| device_error("sleep", e))
    }
}
