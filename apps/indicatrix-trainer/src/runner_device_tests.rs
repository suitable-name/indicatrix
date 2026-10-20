//! Device selection and backend-label regressions, hardware-independent where possible.

use super::*;

#[test]
fn resolved_backend_labels_recreate_the_same_trainer_device() {
    assert_eq!(device_of_label(CPU_LABEL), DeviceChoice::Cpu);
    assert_eq!(device_of_label(GPU_LABEL), DeviceChoice::Gpu);
    assert_eq!(device_of_label(CUDA_LABEL), DeviceChoice::Cuda);
}

#[cfg(not(feature = "cuda"))]
#[test]
fn explicit_cuda_without_the_feature_returns_a_clear_error_without_hardware() {
    let result = build_trainer(
        &indicatrix_ai::training::PpoConfig::default(),
        indicatrix_ai::model::ModelTier::Medium,
        4,
        DeviceChoice::Cuda,
        1,
    );
    let error = match result {
        Err(error) => error.to_string(),
        Ok(_) => panic!("native CUDA cannot initialize without the cargo feature"),
    };
    assert!(error.to_ascii_lowercase().contains("cuda"), "{error}");
    assert!(
        error.contains("feature") || error.contains("--features"),
        "{error}"
    );
}
