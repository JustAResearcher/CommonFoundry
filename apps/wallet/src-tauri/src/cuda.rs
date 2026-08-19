use std::env;
use std::ffi::{CStr, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use cmfd_consensus::{ForgeMatrixV2AcceleratorBatch, ForgeMatrixV2AcceleratorModel};
use libloading::Library;

const CUDA_API_VERSION: u32 = 1;
const ERROR_BUFFER_BYTES: usize = 512;

type ApiVersionFn = unsafe extern "C" fn() -> u32;
type DeviceCountFn = unsafe extern "C" fn(*mut i32, *mut c_char, usize) -> i32;
type DeviceInfoFn = unsafe extern "C" fn(i32, *mut RawDeviceInfo, *mut c_char, usize) -> i32;
type CreateFn = unsafe extern "C" fn(
    i32,
    u32,
    u32,
    u32,
    u32,
    *const u8,
    usize,
    *const u8,
    usize,
    *mut *mut c_void,
    *mut c_char,
    usize,
) -> i32;
type EvaluateFn = unsafe extern "C" fn(
    *mut c_void,
    *const u8,
    usize,
    u32,
    *mut u8,
    usize,
    *mut c_char,
    usize,
) -> i32;
type DestroyFn = unsafe extern "C" fn(*mut c_void);

#[repr(C)]
struct RawDeviceInfo {
    api_version: u32,
    device_index: i32,
    compute_major: u32,
    compute_minor: u32,
    total_memory_bytes: u64,
    name: [c_char; 128],
}

impl Default for RawDeviceInfo {
    fn default() -> Self {
        Self {
            api_version: 0,
            device_index: 0,
            compute_major: 0,
            compute_minor: 0,
            total_memory_bytes: 0,
            name: [0; 128],
        }
    }
}

struct CudaApi {
    api_version: ApiVersionFn,
    device_count: DeviceCountFn,
    device_info: DeviceInfoFn,
    create: CreateFn,
    evaluate: EvaluateFn,
    destroy: DestroyFn,
    _library: Library,
}

impl CudaApi {
    fn load(path: &Path) -> Result<Self, String> {
        // SAFETY: symbols are copied while `library` is alive and the library
        // is retained in the returned API for at least as long as every pointer.
        unsafe {
            let library = Library::new(path)
                .map_err(|error| format!("could not load {}: {error}", path.display()))?;
            let api = Self {
                api_version: load_symbol(&library, b"cmfd_cuda_api_version\0")?,
                device_count: load_symbol(&library, b"cmfd_cuda_device_count\0")?,
                device_info: load_symbol(&library, b"cmfd_cuda_device_info\0")?,
                create: load_symbol(&library, b"cmfd_cuda_create\0")?,
                evaluate: load_symbol(&library, b"cmfd_cuda_evaluate\0")?,
                destroy: load_symbol(&library, b"cmfd_cuda_destroy\0")?,
                _library: library,
            };
            let version = (api.api_version)();
            if version != CUDA_API_VERSION {
                return Err(format!(
                    "CUDA backend API version {version} does not match wallet API {CUDA_API_VERSION}"
                ));
            }
            Ok(api)
        }
    }
}

unsafe fn load_symbol<T: Copy>(library: &Library, symbol: &[u8]) -> Result<T, String> {
    // SAFETY: the caller supplies the exact C ABI type for a versioned backend
    // symbol and retains the library for the lifetime of the copied pointer.
    unsafe {
        library
            .get::<T>(symbol)
            .map(|loaded| *loaded)
            .map_err(|error| {
                format!(
                    "CUDA backend is missing {}: {error}",
                    String::from_utf8_lossy(&symbol[..symbol.len().saturating_sub(1)])
                )
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CudaDevice {
    pub index: i32,
    pub name: String,
    pub compute_major: u32,
    pub compute_minor: u32,
    pub total_memory_bytes: u64,
}

impl CudaDevice {
    pub fn label(&self) -> String {
        format!(
            "{} (CUDA {}.{})",
            self.name, self.compute_major, self.compute_minor
        )
    }
}

pub struct CudaMiner {
    context: NonNull<c_void>,
    api: CudaApi,
    device: CudaDevice,
}

impl CudaMiner {
    /// Loads the optional backend. Absence means CPU fallback; a present but
    /// invalid backend is an error so a broken accelerator is never hidden.
    pub fn load(model: &ForgeMatrixV2AcceleratorModel) -> Result<Option<Self>, String> {
        let explicit = env::var_os("CMFD_CUDA_MINER_LIBRARY").map(PathBuf::from);
        let candidates = library_candidates(explicit.as_deref());
        let path = candidates.iter().find(|path| path.is_file());
        let Some(path) = path else {
            if let Some(path) = explicit {
                return Err(format!(
                    "CMFD_CUDA_MINER_LIBRARY does not name a file: {}",
                    path.display()
                ));
            }
            return Ok(None);
        };

        let api = CudaApi::load(path)?;
        let device_index = selected_device()?;
        let device = read_device(&api, device_index)?;
        let mut context = std::ptr::null_mut();
        let mut error = [0 as c_char; ERROR_BUFFER_BYTES];
        // SAFETY: all buffers remain valid for the duration of the call and
        // lengths exactly describe their allocations. The returned context is
        // owned by this value and destroyed before unloading the library.
        let result = unsafe {
            (api.create)(
                device_index,
                model.rows(),
                model.width(),
                model.layers(),
                model.coefficient_count(),
                model.base_input().as_ptr(),
                model.base_input().len(),
                model.weights().as_ptr(),
                model.weights().len(),
                &mut context,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        check_result(result, &error)?;
        let context = NonNull::new(context)
            .ok_or_else(|| "CUDA backend returned a null context".to_owned())?;
        Ok(Some(Self {
            context,
            api,
            device,
        }))
    }

    pub fn device(&self) -> &CudaDevice {
        &self.device
    }

    pub fn evaluate(&mut self, batch: &ForgeMatrixV2AcceleratorBatch) -> Result<Vec<u8>, String> {
        let output_len = (batch.count() as usize)
            .checked_mul(batch.activation_len())
            .ok_or_else(|| "CUDA output length overflow".to_owned())?;
        let mut outputs = vec![0_u8; output_len];
        let mut error = [0 as c_char; ERROR_BUFFER_BYTES];
        // SAFETY: the context belongs to this backend; input and output buffers
        // remain live and are sized from the private authoritative batch.
        let result = unsafe {
            (self.api.evaluate)(
                self.context.as_ptr(),
                batch.coefficients().as_ptr(),
                batch.coefficients().len(),
                batch.count(),
                outputs.as_mut_ptr(),
                outputs.len(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        check_result(result, &error)?;
        Ok(outputs)
    }
}

impl Drop for CudaMiner {
    fn drop(&mut self) {
        // SAFETY: this is the unique context returned by `create`; destroy is
        // called exactly once while the backing library remains loaded.
        unsafe { (self.api.destroy)(self.context.as_ptr()) };
    }
}

fn read_device(api: &CudaApi, device_index: i32) -> Result<CudaDevice, String> {
    let mut count = 0_i32;
    let mut error = [0 as c_char; ERROR_BUFFER_BYTES];
    // SAFETY: pointers reference writable fixed-size values for the call.
    let result = unsafe { (api.device_count)(&mut count, error.as_mut_ptr(), error.len()) };
    check_result(result, &error)?;
    if device_index < 0 || device_index >= count {
        return Err(format!(
            "CUDA device index {device_index} is outside the available range 0..{count}"
        ));
    }

    let mut raw = RawDeviceInfo::default();
    error.fill(0);
    // SAFETY: `raw` and the error buffer are valid writable C layouts.
    let result =
        unsafe { (api.device_info)(device_index, &mut raw, error.as_mut_ptr(), error.len()) };
    check_result(result, &error)?;
    if raw.api_version != CUDA_API_VERSION {
        return Err("CUDA device response has the wrong API version".to_owned());
    }
    // SAFETY: the backend promises a zero-filled, nul-terminated name buffer.
    let name = unsafe { CStr::from_ptr(raw.name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    Ok(CudaDevice {
        index: raw.device_index,
        name,
        compute_major: raw.compute_major,
        compute_minor: raw.compute_minor,
        total_memory_bytes: raw.total_memory_bytes,
    })
}

fn selected_device() -> Result<i32, String> {
    match env::var("CMFD_CUDA_DEVICE") {
        Ok(value) => value
            .parse::<i32>()
            .map_err(|_| "CMFD_CUDA_DEVICE must be a nonnegative decimal device index".to_owned()),
        Err(env::VarError::NotPresent) => Ok(0),
        Err(env::VarError::NotUnicode(_)) => {
            Err("CMFD_CUDA_DEVICE is not valid Unicode".to_owned())
        }
    }
}

fn library_candidates(explicit: Option<&Path>) -> Vec<PathBuf> {
    if let Some(explicit) = explicit {
        return vec![explicit.to_path_buf()];
    }

    let name = if cfg!(target_os = "windows") {
        "cmfd-forgematrix-v2-miner.dll"
    } else {
        "cmfd-forgematrix-v2-miner.so"
    };
    let mut paths = Vec::new();
    if let Ok(executable) = env::current_exe()
        && let Some(parent) = executable.parent()
    {
        paths.push(parent.join(name));
    }
    if cfg!(debug_assertions) {
        paths.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../target/gpu-miner-build")
                .join(name),
        );
    }
    paths
}

fn check_result(result: i32, error: &[c_char]) -> Result<(), String> {
    if result == 0 {
        return Ok(());
    }
    // SAFETY: every backend call zero-initializes this fixed buffer and writes
    // at most its declared capacity, always retaining a trailing nul byte.
    let message = unsafe { CStr::from_ptr(error.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    Err(if message.is_empty() {
        format!("CUDA backend failed with code {result}")
    } else {
        message
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmfd_consensus::{BlockChallenge, v2_test_reference};

    #[test]
    fn explicit_library_path_is_the_only_candidate() {
        let path = Path::new("custom-cuda-backend.dll");
        assert_eq!(library_candidates(Some(path)), vec![path]);
    }

    #[test]
    fn device_label_includes_compute_capability() {
        let device = CudaDevice {
            index: 0,
            name: "Example GPU".to_owned(),
            compute_major: 8,
            compute_minor: 9,
            total_memory_bytes: 1,
        };
        assert_eq!(device.label(), "Example GPU (CUDA 8.9)");
    }

    #[test]
    fn available_cuda_backend_matches_authoritative_v2_digests() {
        let reference = v2_test_reference().unwrap();
        let descriptor = reference.descriptor();
        let block = BlockChallenge {
            network_id: descriptor.network_id,
            previous_block: [1; 32],
            transaction_root: [2; 32],
            height: 3,
            timestamp: 1_700_000_123,
            target: [0xff; 32],
        };
        let model = reference.accelerator_model();
        let Some(mut backend) = CudaMiner::load(&model).unwrap() else {
            return;
        };
        let batch = reference
            .prepare_accelerator_batch(&block, 91, 128)
            .unwrap();
        let outputs = backend.evaluate(&batch).unwrap();
        for (index, output) in outputs.chunks_exact(batch.activation_len()).enumerate() {
            let accelerated = batch.candidate_work_digest(index, output).unwrap();
            let reference = reference
                .prove_compact(&block, batch.nonce_at(index).unwrap())
                .unwrap();
            assert_eq!(accelerated, reference.work_digest, "nonce index {index}");
        }
    }
}
