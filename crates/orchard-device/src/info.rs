use orchard_core::{Error, Result, Udid};
use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};

use crate::connect::DeviceConnection;
use crate::error::device_error;

/// Capacity of the user data partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Storage {
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// What lockdown reports about a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub udid: Udid,
    pub name: String,
    /// Hardware identifier such as `iPhone15,2`.
    pub product_type: String,
    /// Marketing name such as `iPhone 14 Pro`, when the product type is known.
    pub product_name: Option<String>,
    /// `iPhone`, `iPad`, `iPod` or `Watch`.
    pub device_class: Option<String>,
    /// Board identifier such as `D73AP`.
    pub hardware_model: Option<String>,
    /// Retail part number such as `MQ0T3LL/A` (model number plus region).
    pub model_number: Option<String>,
    pub device_color: Option<String>,
    pub enclosure_color: Option<String>,
    pub os_version: String,
    pub build_version: String,
    pub serial_number: Option<String>,
    pub wifi_address: Option<String>,
    pub bluetooth_address: Option<String>,
    pub phone_number: Option<String>,
    pub imei: Option<String>,
    pub cpu_architecture: Option<String>,
    pub activation_state: Option<String>,
    /// Percent, 0 to 100.
    pub battery_level: Option<u8>,
    pub battery_charging: Option<bool>,
    pub storage: Option<Storage>,
    pub wifi_sync_enabled: Option<bool>,
}

impl DeviceInfo {
    /// Builds the struct from lockdown values: the global domain, then the
    /// `com.apple.mobile.battery` and `com.apple.disk_usage` domains and the
    /// `EnableWifiConnections` value when they could be read.
    pub(crate) fn from_lockdown(
        global: &Dictionary,
        battery: Option<&Dictionary>,
        disk: Option<&Dictionary>,
        wifi_connections: Option<&Value>,
    ) -> Result<Self> {
        let required = |key: &str| {
            string(global, key)
                .ok_or_else(|| Error::Protocol(format!("lockdown values missing {key}")))
        };
        let model_number =
            string(global, "ModelNumber").map(|model| match string(global, "RegionInfo") {
                Some(region) if !model.ends_with(&region) => format!("{model}{region}"),
                _ => model,
            });
        let storage = disk.and_then(|d| {
            let total = uint(d, "TotalDiskCapacity").or_else(|| uint(d, "TotalDataCapacity"))?;
            let free = uint(d, "AmountDataAvailable").or_else(|| uint(d, "TotalDataAvailable"))?;
            Some(Storage {
                total_bytes: total,
                free_bytes: free,
            })
        });
        Ok(Self {
            udid: Udid(required("UniqueDeviceID")?),
            name: string(global, "DeviceName").unwrap_or_default(),
            product_name: string(global, "ProductType")
                .and_then(|t| crate::models::product_name(&t))
                .map(str::to_string),
            product_type: required("ProductType")?,
            device_class: string(global, "DeviceClass"),
            hardware_model: string(global, "HardwareModel"),
            model_number,
            device_color: string(global, "DeviceColor"),
            enclosure_color: string(global, "DeviceEnclosureColor"),
            os_version: required("ProductVersion")?,
            build_version: string(global, "BuildVersion").unwrap_or_default(),
            serial_number: string(global, "SerialNumber"),
            wifi_address: string(global, "WiFiAddress"),
            bluetooth_address: string(global, "BluetoothAddress"),
            phone_number: string(global, "PhoneNumber"),
            imei: string(global, "InternationalMobileEquipmentIdentity"),
            cpu_architecture: string(global, "CPUArchitecture"),
            activation_state: string(global, "ActivationState"),
            battery_level: battery
                .and_then(|b| uint(b, "BatteryCurrentCapacity"))
                .map(|level| level.min(100) as u8),
            battery_charging: battery.and_then(|b| b.get("BatteryIsCharging")?.as_boolean()),
            storage,
            wifi_sync_enabled: wifi_connections.and_then(Value::as_boolean),
        })
    }
}

pub(crate) fn string(dict: &Dictionary, key: &str) -> Option<String> {
    dict.get(key)?
        .as_string()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(crate) fn uint(dict: &Dictionary, key: &str) -> Option<u64> {
    match dict.get(key)? {
        Value::Integer(i) => i.as_unsigned(),
        Value::Real(r) if *r >= 0.0 => Some(*r as u64),
        _ => None,
    }
}

impl DeviceConnection {
    /// Reads identity, OS, battery, storage and Wi-Fi settings from lockdown.
    pub async fn info(&self) -> Result<DeviceInfo> {
        let mut lockdown = self.lockdown().await?;
        let global = lockdown
            .get_value(None, None)
            .await
            .map_err(|e| device_error("lockdown values", e))?;
        let global = global
            .into_dictionary()
            .ok_or_else(|| Error::Protocol("lockdown values are not a dictionary".into()))?;
        let battery = lockdown
            .get_value(None, Some("com.apple.mobile.battery"))
            .await
            .ok()
            .and_then(Value::into_dictionary);
        let disk = lockdown
            .get_value(None, Some("com.apple.disk_usage"))
            .await
            .ok()
            .and_then(Value::into_dictionary);
        let wifi = lockdown
            .get_value(
                Some("EnableWifiConnections"),
                Some("com.apple.mobile.wireless_lockdown"),
            )
            .await
            .ok();
        DeviceInfo::from_lockdown(&global, battery.as_ref(), disk.as_ref(), wifi.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLOBAL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>ActivationState</key><string>Activated</string>
  <key>BasebandVersion</key><string>2.20.03</string>
  <key>BluetoothAddress</key><string>a4:83:e7:00:11:22</string>
  <key>BuildVersion</key><string>22B83</string>
  <key>CPUArchitecture</key><string>arm64e</string>
  <key>DeviceClass</key><string>iPhone</string>
  <key>DeviceColor</key><string>1</string>
  <key>DeviceEnclosureColor</key><string>4</string>
  <key>DeviceName</key><string>Office iPhone</string>
  <key>HardwareModel</key><string>D73AP</string>
  <key>InternationalMobileEquipmentIdentity</key><string>356789012345678</string>
  <key>ModelNumber</key><string>MQ0T3</string>
  <key>PhoneNumber</key><string>+39 333 000 0000</string>
  <key>ProductType</key><string>iPhone15,2</string>
  <key>ProductVersion</key><string>18.1</string>
  <key>RegionInfo</key><string>LL/A</string>
  <key>SerialNumber</key><string>F2LXK0Q1PQ</string>
  <key>UniqueDeviceID</key><string>00008120-001A2B3C4D5E6F70</string>
  <key>WiFiAddress</key><string>a4:83:e7:00:11:23</string>
  <key>ProductionSOC</key><true/>
</dict>
</plist>"#;

    const BATTERY: &str = r#"<plist version="1.0"><dict>
  <key>BatteryCurrentCapacity</key><integer>87</integer>
  <key>BatteryIsCharging</key><true/>
  <key>ExternalChargeCapable</key><true/>
  <key>ExternalConnected</key><true/>
  <key>FullyCharged</key><false/>
  <key>GasGaugeCapability</key><true/>
  <key>HasBattery</key><true/>
</dict></plist>"#;

    const DISK: &str = r#"<plist version="1.0"><dict>
  <key>AmountDataAvailable</key><integer>61203456000</integer>
  <key>AmountDataReserved</key><integer>209715200</integer>
  <key>CalculateDiskUsage</key><string>OkilyDokily</string>
  <key>TotalDataAvailable</key><integer>61413171200</integer>
  <key>TotalDataCapacity</key><integer>118410969088</integer>
  <key>TotalDiskCapacity</key><integer>128000000000</integer>
  <key>TotalSystemAvailable</key><integer>0</integer>
  <key>TotalSystemCapacity</key><integer>9589030912</integer>
</dict></plist>"#;

    fn dict(xml: &str) -> Dictionary {
        plist::from_bytes(xml.as_bytes()).unwrap()
    }

    #[test]
    fn parses_full_lockdown_values() {
        let info = DeviceInfo::from_lockdown(
            &dict(GLOBAL),
            Some(&dict(BATTERY)),
            Some(&dict(DISK)),
            Some(&Value::Boolean(true)),
        )
        .unwrap();
        assert_eq!(info.udid.0, "00008120-001A2B3C4D5E6F70");
        assert_eq!(info.name, "Office iPhone");
        assert_eq!(info.product_type, "iPhone15,2");
        assert_eq!(info.product_name.as_deref(), Some("iPhone 14 Pro"));
        assert_eq!(info.device_class.as_deref(), Some("iPhone"));
        assert_eq!(info.model_number.as_deref(), Some("MQ0T3LL/A"));
        assert_eq!(info.os_version, "18.1");
        assert_eq!(info.build_version, "22B83");
        assert_eq!(info.serial_number.as_deref(), Some("F2LXK0Q1PQ"));
        assert_eq!(info.imei.as_deref(), Some("356789012345678"));
        assert_eq!(info.battery_level, Some(87));
        assert_eq!(info.battery_charging, Some(true));
        assert_eq!(
            info.storage,
            Some(Storage {
                total_bytes: 128_000_000_000,
                free_bytes: 61_203_456_000
            })
        );
        assert_eq!(info.wifi_sync_enabled, Some(true));
    }

    #[test]
    fn optional_domains_may_be_missing() {
        let mut global = dict(GLOBAL);
        global.remove("RegionInfo");
        global.remove("PhoneNumber");
        let info = DeviceInfo::from_lockdown(&global, None, None, None).unwrap();
        assert_eq!(info.model_number.as_deref(), Some("MQ0T3"));
        assert_eq!(info.phone_number, None);
        assert_eq!(info.battery_level, None);
        assert_eq!(info.storage, None);
        assert_eq!(info.wifi_sync_enabled, None);
    }

    #[test]
    fn missing_identity_is_a_protocol_error() {
        let mut global = dict(GLOBAL);
        global.remove("ProductVersion");
        let err = DeviceInfo::from_lockdown(&global, None, None, None).unwrap_err();
        assert_eq!(err.kind(), "protocol");
    }

    #[test]
    fn serializes_camel_case() {
        let info = DeviceInfo::from_lockdown(&dict(GLOBAL), None, None, None).unwrap();
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["productType"], "iPhone15,2");
        assert_eq!(json["osVersion"], "18.1");
        assert!(json.get("wifiSyncEnabled").is_some());
    }
}
