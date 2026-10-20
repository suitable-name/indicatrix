//! Explicit backend constructors for [`BurnTrainer`].
//!
//! CUDA is an opt-in native CubeCL backend. The ordinary package features and the existing
//! WGPU/CPU constructors stay independent of CUDA availability.

use burn::{prelude::*, tensor::TensorData};
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::{
    PpoConfig, TrainError,
    burn_model::{BurnSetAttention, host_vec},
    burn_ppo::{BurnTrainer, MAX_TIERS_LIMIT, ModelShape},
};

impl BurnTrainer {
    /// Create a trainer targeting the GPU through WebGPU / Vulkan / DX12.
    ///
    /// # Errors
    ///
    /// [`TrainError::DeviceUnavailable`] when no usable adapter exists (checked with a one-element
    /// tensor operation); there is no silent fallback. Use [`Self::new_auto`] to opt into one.
    pub fn new_gpu(shape: ModelShape, config: PpoConfig) -> Result<Self, TrainError> {
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            let device = Device::wgpu(burn::tensor::DeviceKind::default());
            probe_device(&device).map(|()| device)
        }));
        match attempt {
            Ok(Ok(device)) => Self::new(shape, config, device),
            Ok(Err(e)) => Err(e),
            Err(payload) => Err(TrainError::DeviceUnavailable(panic_message(
                &payload,
                "panic while initialising the GPU backend",
            ))),
        }
    }

    /// Create a trainer targeting CPU SIMD (Flex).
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn new_cpu(shape: ModelShape, config: PpoConfig) -> Result<Self, TrainError> {
        Self::new(shape, config, Device::flex())
    }

    /// Create a trainer on WGPU when it works, otherwise on CPU.
    ///
    /// The second tuple element carries the reason WGPU was skipped (`None` when it is in use);
    /// the caller is expected to log it as a warning. This method does not probe CUDA.
    ///
    /// # Errors
    ///
    /// As [`Self::new`]; a WGPU failure alone is not an error here.
    pub fn new_auto(
        shape: ModelShape,
        config: PpoConfig,
    ) -> Result<(Self, Option<TrainError>), TrainError> {
        match Self::new_gpu(shape, config.clone()) {
            Ok(t) => Ok((t, None)),
            Err(gpu_err @ TrainError::DeviceUnavailable(_)) => {
                Self::new_cpu(shape, config).map(|t| (t, Some(gpu_err)))
            }
            Err(other) => Err(other),
        }
    }

    /// Create a trainer using native CUDA device index 0.
    ///
    /// This explicit constructor never falls back to WGPU or CPU. CUDA support is optional and can
    /// be enabled for this crate with the `cuda` feature (`--features cuda`); when that feature is
    /// absent, this method returns an actionable [`TrainError::DeviceUnavailable`] error. The
    /// selected device is the CUDA hardware index 0 as interpreted by Burn 0.22.
    ///
    /// # Errors
    ///
    /// [`TrainError::InvalidConfig`] for invalid PPO settings, dimensions or tier count;
    /// [`TrainError::DeviceUnavailable`] when the CUDA feature is absent, the device probe fails,
    /// CUDA trainer initialization fails, or backend initialization panics.
    pub fn new_cuda(shape: ModelShape, config: PpoConfig) -> Result<Self, TrainError> {
        validate_cuda_inputs(shape, &config)?;

        #[cfg(not(feature = "cuda"))]
        {
            let _ = (shape, config);
            Err(TrainError::DeviceUnavailable(
                "CUDA support is disabled in this build; rebuild indicatrix-ai with `--features cuda`"
                    .into(),
            ))
        }

        #[cfg(feature = "cuda")]
        {
            let attempt = catch_unwind(AssertUnwindSafe(|| {
                let device = Device::cuda(0);
                probe_cuda_device(&device)?;
                Self::new(shape, config, device).map_err(map_cuda_initialization_error)
            }));
            match attempt {
                Ok(result) => result,
                Err(payload) => Err(TrainError::DeviceUnavailable(format!(
                    "CUDA device 0 panic while probing or initializing: {}",
                    panic_message(&payload, "unknown backend panic")
                ))),
            }
        }
    }
}

/// Validate all user-controlled settings before touching a native backend. That keeps invalid
/// model/configuration errors distinguishable from missing hardware and runtime initialization.
fn validate_cuda_inputs(shape: ModelShape, config: &PpoConfig) -> Result<(), TrainError> {
    config.validate()?;
    if shape.max_tiers == 0 || shape.max_tiers > MAX_TIERS_LIMIT {
        return Err(TrainError::InvalidConfig(format!(
            "max_tiers must be 1..={MAX_TIERS_LIMIT} (got {})",
            shape.max_tiers
        )));
    }
    BurnSetAttention::check_dims(shape.d_tier, shape.d_global, shape.d_model, shape.num_heads)
}

/// A native probe failure is a device-availability problem even if Burn reports it as a generic
/// tensor readback error.
#[cfg(feature = "cuda")]
fn probe_cuda_device(device: &Device) -> Result<(), TrainError> {
    probe_device(device).map_err(|error| {
        TrainError::DeviceUnavailable(format!("CUDA device 0 probe failed: {error}"))
    })?;

    // Exercise a math kernel (the LLVM backend needs CUDA's libdevice for math intrinsics),
    // matrix multiplication and autodiff without warming up the full model.
    let autodiff_device = device.clone().autodiff();
    let input = Tensor::<2>::from_data(
        TensorData::new(vec![0.25f32, 0.5, 0.75, 1.0], [2, 2]),
        &autodiff_device,
    )
    .require_grad();
    let loss = input.clone().sin().matmul(input.clone()).sum();
    let gradients = loss.backward();
    let gradient = input.grad(&gradients).ok_or_else(|| {
        TrainError::DeviceUnavailable("CUDA device 0 autodiff probe returned no gradient".into())
    })?;
    let values = host_vec(gradient).map_err(|error| {
        TrainError::DeviceUnavailable(format!("CUDA device 0 autodiff probe failed: {error}"))
    })?;
    if values.len() == 4
        && values.iter().all(|value| value.is_finite())
        && values.iter().any(|value| *value != 0.0)
    {
        Ok(())
    } else {
        Err(TrainError::DeviceUnavailable(format!(
            "CUDA device 0 autodiff probe returned invalid gradients: {values:?}"
        )))
    }
}

/// Run a one-element tensor operation to prove the selected device works.
fn probe_device(device: &Device) -> Result<(), TrainError> {
    let t =
        Tensor::<2>::from_data(TensorData::new(vec![1.0f32, 2.0], [1, 2]), device).add_scalar(1.0);
    let values = host_vec(t)?;
    if values == [2.0, 3.0] {
        Ok(())
    } else {
        Err(TrainError::DeviceUnavailable(format!(
            "device probe returned {values:?}, expected [2.0, 3.0]"
        )))
    }
}

#[cfg(feature = "cuda")]
fn map_cuda_initialization_error(error: TrainError) -> TrainError {
    match error {
        invalid @ TrainError::InvalidConfig(_) => invalid,
        unavailable @ TrainError::DeviceUnavailable(_) => unavailable,
        other => {
            TrainError::DeviceUnavailable(format!("CUDA trainer initialization failed: {other}"))
        }
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>, fallback: &str) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|value| (*value).to_owned())
        })
        .unwrap_or_else(|| fallback.to_owned())
}
