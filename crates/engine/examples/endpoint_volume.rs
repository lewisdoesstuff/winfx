//! Show mute and volume for active capture endpoints; optionally unmute them.
//!   cargo run -p engine --example endpoint_volume -- [id-substring] [--unmute]
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{DEVICE_STATE_ACTIVE, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};

fn main() -> windows::core::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let unmute = args.iter().any(|a| a == "--unmute");
    let want = args.iter().find(|a| !a.starts_with("--")).cloned().unwrap_or_default();
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let col = en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
        for i in 0..col.GetCount()? {
            let dev = col.Item(i)?;
            let id = dev.GetId()?;
            let name = id.to_string().unwrap_or_default();
            CoTaskMemFree(Some(id.0 as _));
            if !name.contains(&want) {
                continue;
            }
            let vol: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None)?;
            let muted = vol.GetMute()?.as_bool();
            let db = vol.GetMasterVolumeLevel()?;
            let scalar = vol.GetMasterVolumeLevelScalar()?;
            println!("{name}: muted={muted} level={db:.1} dB ({:.0} %)", scalar * 100.0);
            if unmute && muted {
                vol.SetMute(false, std::ptr::null())?;
                println!("  -> unmuted");
            }
        }
    }
    Ok(())
}
