use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use log::{debug, info, warn};

use crate::audio::devices::configuration::{AudioDevice, DeviceType};

/// Configure Windows audio devices using WASAPI
pub fn configure_windows_audio(host: &cpal::Host) -> Result<Vec<AudioDevice>> {
    let mut devices = Vec::new();

    // Get WASAPI devices
    if let Ok(wasapi_host) = cpal::host_from_id(cpal::HostId::Wasapi) {
        debug!("Using WASAPI host for Windows audio device enumeration");

        // Add output devices (including loopback)
        if let Ok(output_devices) = wasapi_host.output_devices() {
            for device in output_devices {
                if let Ok(name) = device.name() {
                    // For Windows, we need to mark output devices specifically for loopback
                    // info!("Found Windows output device: {}", name);
                    push_unique_device(&mut devices, name.clone(), DeviceType::Output);
                }
            }
        } else {
            warn!("Failed to enumerate WASAPI output devices");
        }

        // Add input devices from WASAPI
        if let Ok(input_devices) = wasapi_host.input_devices() {
            for device in input_devices {
                if let Ok(name) = device.name() {
                    // info!("Found Windows input device: {}", name);
                    push_unique_device(&mut devices, name.clone(), DeviceType::Input);
                }
            }
        } else {
            warn!("Failed to enumerate WASAPI input devices");
        }
    } else {
        warn!("Failed to create WASAPI host, falling back to default host");
    }

    // If WASAPI failed or returned no devices, try default host as fallback
    if devices.is_empty() {
        debug!("WASAPI device enumeration failed or returned no devices, falling back to default host");
        // Add regular input devices
        if let Ok(input_devices) = host.input_devices() {
            for device in input_devices {
                if let Ok(name) = device.name() {
                    // info!("Found fallback input device: {}", name);
                    push_unique_device(&mut devices, name.clone(), DeviceType::Input);
                }
            }
        } else {
            warn!("Failed to enumerate input devices from default host");
        }

        // Add output devices
        if let Ok(output_devices) = host.output_devices() {
            for device in output_devices {
                if let Ok(name) = device.name() {
                    // info!("Found fallback output device: {}", name);
                    push_unique_device(&mut devices, name.clone(), DeviceType::Output);
                }
            }
        } else {
            warn!("Failed to enumerate output devices from default host");
        }
    }

    // If we still have no devices, add default devices
    if devices.is_empty() {
        warn!("No audio devices found, adding default devices only");

        // Try to add default input device
        if let Some(device) = host.default_input_device() {
            if let Ok(name) = device.name() {
                // info!("Adding default input device: {}", name);
                push_unique_device(&mut devices, name, DeviceType::Input);
            }
        }

        // Try to add default output device
        if let Some(device) = host.default_output_device() {
            if let Ok(name) = device.name() {
                // info!("Adding default output device: {}", name);
                push_unique_device(&mut devices, name, DeviceType::Output);
            }
        }
    }

    debug!("Found {} Windows audio devices", devices.len());
    Ok(devices)
}

/// Add a device unless the same name and type is already listed.
///
/// WASAPI can enumerate an endpoint more than once, and the interface would then
/// offer indistinguishable duplicates.
fn push_unique_device(devices: &mut Vec<AudioDevice>, name: String, device_type: DeviceType) {
    let device = AudioDevice::new(name, device_type);
    if !devices.contains(&device) {
        devices.push(device);
    }
}

/// Rank how well a candidate endpoint name matches the requested base name.
///
/// Lower is better. The last tier is a substring test, which is how a stored name
/// that no longer matches exactly still resolves — and why the tiers must be ranked
/// instead of accepted in enumeration order.
fn name_match_rank(candidate: &str, wanted: &str) -> Option<u8> {
    if candidate == wanted {
        Some(0)
    } else if candidate.eq_ignore_ascii_case(wanted) {
        Some(1)
    } else if candidate.contains(wanted) {
        Some(2)
    } else {
        None
    }
}

/// Indices of endpoints matching `wanted`, best tier first and enumeration order
/// within a tier.
fn ranked_match_indices(names: &[String], wanted: &str) -> Vec<usize> {
    let mut matches: Vec<(u8, usize)> = names
        .iter()
        .enumerate()
        .filter_map(|(index, name)| name_match_rank(name, wanted).map(|rank| (rank, index)))
        .collect();
    matches.sort_by_key(|(rank, index)| (*rank, *index));
    matches.into_iter().map(|(_, index)| index).collect()
}

/// Turn a supported range into a config, preferring the pipeline's 48 kHz.
///
/// The rest of the capture path runs at 48 kHz, so asking for it here avoids a
/// resample; a range that does not include 48 kHz keeps the old maximum-rate choice.
fn preferred_config(range: cpal::SupportedStreamConfigRange) -> cpal::SupportedStreamConfig {
    const PIPELINE_SAMPLE_RATE: u32 = 48_000;
    let supports_pipeline_rate = range.min_sample_rate().0 <= PIPELINE_SAMPLE_RATE
        && PIPELINE_SAMPLE_RATE <= range.max_sample_rate().0;
    if supports_pipeline_rate {
        range.with_sample_rate(cpal::SampleRate(PIPELINE_SAMPLE_RATE))
    } else {
        range.with_max_sample_rate()
    }
}

/// First F32 stereo range, else the first F32 range, else the first range.
fn choose_config(
    configs: &[cpal::SupportedStreamConfigRange],
) -> Option<cpal::SupportedStreamConfig> {
    let chosen = configs
        .iter()
        .find(|config| {
            config.sample_format() == cpal::SampleFormat::F32 && config.channels() == 2
        })
        .or_else(|| configs.iter().find(|config| config.sample_format() == cpal::SampleFormat::F32))
        .or_else(|| configs.first())?;
    Some(preferred_config(*chosen))
}

/// Input config for one endpoint: the endpoint default first, then its ranges.
fn resolve_input_config(
    device: &cpal::Device,
    name: &str,
) -> Option<cpal::SupportedStreamConfig> {
    match device.default_input_config() {
        Ok(default_config) => Some(default_config),
        Err(e) => {
            warn!(
                "Failed to get default input config for '{}': {}. Trying supported configs...",
                name, e
            );
            let configs: Vec<_> = match device.supported_input_configs() {
                Ok(ranges) => ranges.collect(),
                Err(e) => {
                    warn!(
                        "Could not enumerate supported configurations for device '{}': {}",
                        name, e
                    );
                    return None;
                }
            };
            if configs.is_empty() {
                warn!("No supported input configurations found for device: {}", name);
                return None;
            }
            choose_config(&configs)
        }
    }
}

/// Output config for one endpoint: its ranges first, then the endpoint default.
fn resolve_output_config(
    device: &cpal::Device,
    name: &str,
) -> Option<cpal::SupportedStreamConfig> {
    let config = match device.supported_output_configs() {
        Ok(ranges) => {
            let configs: Vec<_> = ranges.collect();
            if configs.is_empty() {
                warn!("No supported output configurations found for device: {}", name);
                None
            } else {
                choose_config(&configs)
            }
        }
        Err(e) => {
            warn!(
                "Could not enumerate supported configurations for device '{}': {}",
                name, e
            );
            None
        }
    };
    config.or_else(|| device.default_output_config().ok())
}

/// Get Windows device and configuration using WASAPI
pub fn get_windows_device(audio_device: &AudioDevice) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let wasapi_host = cpal::host_from_id(cpal::HostId::Wasapi)
        .map_err(|e| anyhow!("Failed to create WASAPI host: {}", e))?;

    // Extract the base device name without the (input) or (output) suffix
    let base_name = if audio_device.name.ends_with(" (input)") {
        audio_device.name.trim_end_matches(" (input)")
    } else if audio_device.name.ends_with(" (output)") {
        audio_device.name.trim_end_matches(" (output)")
    } else {
        &audio_device.name
    };

    info!("Looking for Windows device with base name: {}", base_name);

    match audio_device.device_type {
        DeviceType::Input => {
            let endpoints: Vec<(cpal::Device, String)> = wasapi_host
                .input_devices()?
                .filter_map(|device| device.name().ok().map(|name| (device, name)))
                .collect();
            let names: Vec<String> = endpoints.iter().map(|(_, name)| name.clone()).collect();

            let matches = ranked_match_indices(&names, base_name);
            for index in &matches {
                let (device, name) = &endpoints[*index];
                info!("Checking input device: {}", name);
                if let Some(config) = resolve_input_config(device, name) {
                    return Ok((device.clone(), config));
                }
            }
            if !matches.is_empty() {
                return Err(anyhow!(
                    "No compatible input configuration found for device: {}",
                    base_name
                ));
            }

            // If we didn't find a matching device, try the default input device as fallback
            info!("No matching input device found, trying default input device");
            if let Some(default_device) = wasapi_host.default_input_device() {
                if let Ok(name) = default_device.name() {
                    info!("Using default input device: {}", name);
                    if let Ok(config) = default_device.default_input_config() {
                        return Ok((default_device, config));
                    } else if let Ok(supported_configs) = default_device.supported_input_configs() {
                        if let Some(config) = supported_configs.into_iter().next() {
                            return Ok((default_device, preferred_config(config)));
                        }
                    }
                }
            }
        }
        DeviceType::Output => {
            let endpoints: Vec<(cpal::Device, String)> = wasapi_host
                .output_devices()?
                .filter_map(|device| device.name().ok().map(|name| (device, name)))
                .collect();
            let names: Vec<String> = endpoints.iter().map(|(_, name)| name.clone()).collect();

            for index in ranked_match_indices(&names, base_name) {
                let (device, name) = &endpoints[index];
                info!("Checking output device: {}", name);
                if let Some(config) = resolve_output_config(device, name) {
                    return Ok((device.clone(), config));
                }
            }

            // If we didn't find a matching device, try the default output device as fallback
            info!("No matching output device found, trying default output device");
            if let Some(default_device) = wasapi_host.default_output_device() {
                if let Ok(name) = default_device.name() {
                    info!("Using default output device: {}", name);
                    if let Ok(config) = default_device.default_output_config() {
                        return Ok((default_device, config));
                    } else if let Ok(supported_configs) = default_device.supported_output_configs() {
                        if let Some(config) = supported_configs.into_iter().next() {
                            return Ok((default_device, preferred_config(config)));
                        }
                    }
                }
            }
        }
    }

    Err(anyhow!("Device not found or no compatible configuration available: {}", audio_device.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_match_prefers_exact_then_case_insensitive_then_substring() {
        assert_eq!(
            name_match_rank("Speakers (Realtek)", "Speakers (Realtek)"),
            Some(0)
        );
        assert_eq!(
            name_match_rank("speakers (realtek)", "Speakers (Realtek)"),
            Some(1)
        );
        assert_eq!(
            name_match_rank("Speakers (Realtek) [Loopback]", "Speakers (Realtek)"),
            Some(2)
        );
        assert_eq!(name_match_rank("Microphone Array", "Speakers (Realtek)"), None);
    }

    #[test]
    fn an_exact_match_wins_even_when_a_substring_match_comes_first() {
        // "Mic" is contained in both longer endpoints; the endpoint literally named
        // "Mic" must still be chosen over whichever was enumerated first.
        let names = vec![
            "Microphone (Realtek)".to_string(),
            "Microphone Array".to_string(),
            "Mic".to_string(),
        ];
        assert_eq!(ranked_match_indices(&names, "Mic"), vec![2, 0, 1]);
    }

    #[test]
    fn case_insensitive_exact_beats_an_earlier_substring() {
        let names = vec![
            "Headphones (USB) [2]".to_string(),
            "headphones (usb)".to_string(),
        ];
        assert_eq!(ranked_match_indices(&names, "Headphones (USB)"), vec![1, 0]);
    }

    #[test]
    fn duplicate_endpoints_are_listed_once() {
        let mut devices = Vec::new();
        push_unique_device(&mut devices, "Microphone (USB)".to_string(), DeviceType::Input);
        push_unique_device(&mut devices, "Microphone (USB)".to_string(), DeviceType::Input);
        push_unique_device(&mut devices, "Microphone (USB)".to_string(), DeviceType::Output);
        assert_eq!(devices.len(), 2);
    }
}
