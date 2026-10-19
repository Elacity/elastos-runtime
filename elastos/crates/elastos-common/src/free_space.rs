//! The one free-space rule for user-facing writes.

/// Free space a user-facing write keeps on its volume on top of its own bytes,
/// so a disk is never filled to zero. Update staging, the installer, model and
/// content downloads and Browser image preparation all use it;
/// `scripts/install.sh` repeats the value and a test keeps the two equal.
pub const FREE_SPACE_RESERVE_BYTES: u64 = 2 * GIB as u64;

const GIB: u128 = 1 << 30;

/// The write's bytes plus [`FREE_SPACE_RESERVE_BYTES`] do not fit in the
/// volume's available space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotEnoughFreeSpace {
    pub needed: u128,
}

impl std::fmt::Display for NotEnoughFreeSpace {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Round up to a tenth of a GB so a small write never reads as "0 GB".
        let tenths = self.needed.saturating_mul(10).div_ceil(GIB);
        let (whole, tenth) = (tenths / 10, tenths % 10);
        let needed = if tenth == 0 {
            whole.to_string()
        } else {
            format!("{whole}.{tenth}")
        };
        let reserve = u128::from(FREE_SPACE_RESERVE_BYTES) / GIB;
        write!(
            formatter,
            "not enough free space: this needs {needed} GB plus {reserve} GB kept free"
        )
    }
}

impl std::error::Error for NotEnoughFreeSpace {}

/// Admits a write of `needed` bytes only when `needed + FREE_SPACE_RESERVE_BYTES <= available`.
pub fn require_free_space(available: u128, needed: u128) -> Result<(), NotEnoughFreeSpace> {
    let fits = needed
        .checked_add(u128::from(FREE_SPACE_RESERVE_BYTES))
        .is_some_and(|total| total <= available);
    if !fits {
        return Err(NotEnoughFreeSpace { needed });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_fits_only_with_the_reserve_kept_free() {
        let reserve = u128::from(FREE_SPACE_RESERVE_BYTES);
        for needed in [0, 1, 5 * GIB / 2, u128::from(u64::MAX)] {
            assert!(require_free_space(needed + reserve, needed).is_ok());
            assert_eq!(
                require_free_space(needed + reserve - 1, needed),
                Err(NotEnoughFreeSpace { needed })
            );
        }
        // Volume size never matters, and the sum cannot wrap into a fit.
        assert!(require_free_space(u128::MAX, u128::MAX).is_err());
    }

    #[test]
    fn refusal_states_the_bytes_and_the_reserve_in_gb() {
        for (needed, text) in [
            (3 * GIB, "3"),
            (5 * GIB / 2, "2.5"),
            (64 * 1024, "0.1"),
            (0, "0"),
        ] {
            assert_eq!(
                NotEnoughFreeSpace { needed }.to_string(),
                format!("not enough free space: this needs {text} GB plus 2 GB kept free")
            );
        }
    }

    #[test]
    fn installer_keeps_the_same_free_space_reserve() {
        let installer = include_str!("../../../../scripts/install.sh");
        let line = format!("FREE_SPACE_RESERVE_BYTES={FREE_SPACE_RESERVE_BYTES}\n");
        assert!(
            installer.contains(&line),
            "install.sh must keep FREE_SPACE_RESERVE_BYTES equal to the Runtime's"
        );
    }
}
