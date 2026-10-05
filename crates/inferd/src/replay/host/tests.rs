use super::*;
use engine_supervisor::{FakeGpu, GpuError};

struct NoGpu;

impl GpuProbe for NoGpu {
    async fn memory(&self) -> Result<GpuMemory, GpuError> {
        Err(GpuError::Unavailable)
    }
}

#[tokio::test]
async fn spare_memory_stands_in_only_when_the_probe_fails() {
    let real = GpuMemory {
        total: MiB(16_000),
        used_by_others: MiB(10),
    };
    assert_eq!(
        SpareGpu::new(FakeGpu(real), Some(MiB(1))).memory().await,
        Ok(real)
    );
    assert_eq!(
        SpareGpu::new(NoGpu, None).memory().await,
        Err(GpuError::Unavailable)
    );
    let spare = SpareGpu::new(NoGpu, Some(MiB(99)))
        .memory()
        .await
        .expect("spare");
    assert_eq!(spare.total, MiB(99));
}
