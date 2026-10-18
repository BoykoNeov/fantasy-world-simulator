//! Order-independent randomness.
//!
//! Every random number is a hash of `(seed, module, day, entity, purpose)`. Nothing is drawn from a
//! shared stream, so results don't depend on the order parallel work runs in, and adding a module or
//! a draw doesn't shift anyone else's numbers.

/// Which part of the simulation a draw belongs to. Values are fixed forever: changing one changes
/// every world that uses it. Add new ones at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Module {
    Terrain = 1,
    Climate = 2,
    Provinces = 3,
    Pops = 4,
    /// Economics v0 (food). Harvests drew from `Pops` before the module split.
    Economics = 5,
    /// The player's edits (the bumps a roughening brush adds).
    Editor = 6,
    /// Realms: where they start, their names, their claims.
    Realms = 7,
    /// Characters: rulers' and heirs' births, deaths, traits and names.
    Characters = 8,
    /// Warfare: soldiers' deaths and desertions.
    Warfare = 9,
    /// Diplomacy: realms choosing war.
    Diplomacy = 10,
    /// Research: which tech a realm studies next.
    Research = 11,
    /// Culture: where cultures start, people taking them up, splits, names.
    Culture = 12,
    /// Religion: where faiths start, conversion, schisms, names.
    Religion = 13,
    /// Disease: where plagues begin and where they spread.
    Disease = 14,
}

/// SplitMix64 finaliser: a good 64-bit mixer.
pub fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// FNV-1a over the bytes of a purpose name, so purposes are readable strings at the call site.
pub const fn purpose(name: &str) -> u64 {
    let bytes = name.as_bytes();
    let mut h = 0xCBF2_9CE4_8422_2325u64;
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
        i += 1;
    }
    h
}

/// The one random function: a 64-bit hash of all five keys.
pub fn draw(seed: u64, module: Module, day: u64, entity: u64, purpose: u64) -> u64 {
    let mut h = mix(seed);
    for k in [module as u64, day, entity, purpose] {
        h = mix(h ^ k);
    }
    h
}

/// Uniform in [0, 1).
pub fn unit(seed: u64, module: Module, day: u64, entity: u64, purpose: u64) -> f64 {
    (draw(seed, module, day, entity, purpose) >> 11) as f64 / (1u64 << 53) as f64
}

/// Round `x >= 0` to an integer, up with probability equal to its fraction, so expected values are
/// kept exactly while the result stays a whole number.
pub fn round_stochastic(x: f64, seed: u64, module: Module, day: u64, entity: u64, purpose: u64) -> u64 {
    debug_assert!(x >= 0.0 && x.is_finite(), "{x}");
    let whole = x.floor();
    let up = unit(seed, module, day, entity, purpose) < x - whole;
    whole as u64 + up as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_changes_the_draw() {
        let base = draw(1, Module::Pops, 2, 3, 4);
        assert_ne!(base, draw(9, Module::Pops, 2, 3, 4));
        assert_ne!(base, draw(1, Module::Terrain, 2, 3, 4));
        assert_ne!(base, draw(1, Module::Pops, 9, 3, 4));
        assert_ne!(base, draw(1, Module::Pops, 2, 9, 4));
        assert_ne!(base, draw(1, Module::Pops, 2, 3, 9));
        assert_eq!(base, draw(1, Module::Pops, 2, 3, 4));
    }

    #[test]
    fn unit_is_roughly_uniform() {
        let mut buckets = [0u32; 10];
        for e in 0..100_000 {
            buckets[(unit(7, Module::Pops, 0, e, purpose("test")) * 10.0) as usize] += 1;
        }
        for b in buckets {
            assert!((9_500..10_500).contains(&b), "{buckets:?}");
        }
    }

    #[test]
    fn stochastic_rounding_keeps_the_mean() {
        let total: u64 = (0..100_000).map(|e| round_stochastic(2.3, 7, Module::Pops, 0, e, purpose("r"))).sum();
        let mean = total as f64 / 100_000.0;
        assert!((mean - 2.3).abs() < 0.01, "{mean}");
    }
}
