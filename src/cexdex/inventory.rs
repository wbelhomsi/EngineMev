//! Inventory tracking with reservation lifecycle and ratio-based gates.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::cexdex::route::ArbDirection;
use crate::cexdex::units::{atoms_to_usdc, lamports_to_sol};

const DEFAULT_HARD_CAP: f64 = 0.80;
const DEFAULT_PREFERRED_LOW: f64 = 0.40;
const DEFAULT_PREFERRED_HIGH: f64 = 0.60;
const DEFAULT_SKEWED_MULTIPLIER: f64 = 2.0;

#[derive(Clone)]
pub struct Inventory {
    inner: Arc<InventoryInner>,
}

struct InventoryInner {
    sol_on_chain_lamports: AtomicU64,
    usdc_on_chain_atoms: AtomicU64,
    sol_reserved_lamports: AtomicU64,
    usdc_reserved_atoms: AtomicU64,
    sol_price_usd_scaled: AtomicU64, // price * 1e6 for u64 storage

    hard_cap: f64,
    preferred_low: f64,
    preferred_high: f64,
    skewed_multiplier: f64,
}

impl Inventory {
    pub fn new(
        hard_cap: f64,
        preferred_low: f64,
        preferred_high: f64,
        skewed_multiplier: f64,
    ) -> Self {
        Self {
            inner: Arc::new(InventoryInner {
                sol_on_chain_lamports: AtomicU64::new(0),
                usdc_on_chain_atoms: AtomicU64::new(0),
                sol_reserved_lamports: AtomicU64::new(0),
                usdc_reserved_atoms: AtomicU64::new(0),
                sol_price_usd_scaled: AtomicU64::new(0),
                hard_cap,
                preferred_low,
                preferred_high,
                skewed_multiplier,
            }),
        }
    }

    /// Test helper — uses default gates.
    pub fn new_for_test() -> Self {
        Self::new(
            DEFAULT_HARD_CAP,
            DEFAULT_PREFERRED_LOW,
            DEFAULT_PREFERRED_HIGH,
            DEFAULT_SKEWED_MULTIPLIER,
        )
    }

    pub fn set_on_chain(&self, sol_lamports: u64, usdc_atoms: u64) {
        self.inner.sol_on_chain_lamports.store(sol_lamports, Ordering::SeqCst);
        self.inner.usdc_on_chain_atoms.store(usdc_atoms, Ordering::SeqCst);
    }

    pub fn set_sol_price_usd(&self, price: f64) {
        let scaled = (price * 1_000_000.0) as u64;
        self.inner.sol_price_usd_scaled.store(scaled, Ordering::SeqCst);
    }

    pub fn sol_price_usd(&self) -> f64 {
        self.inner.sol_price_usd_scaled.load(Ordering::SeqCst) as f64 / 1_000_000.0
    }

    pub fn sol_lamports_available(&self) -> u64 {
        let on_chain = self.inner.sol_on_chain_lamports.load(Ordering::SeqCst);
        let reserved = self.inner.sol_reserved_lamports.load(Ordering::SeqCst);
        on_chain.saturating_sub(reserved)
    }

    pub fn usdc_atoms_available(&self) -> u64 {
        let on_chain = self.inner.usdc_on_chain_atoms.load(Ordering::SeqCst);
        let reserved = self.inner.usdc_reserved_atoms.load(Ordering::SeqCst);
        on_chain.saturating_sub(reserved)
    }

    pub fn ratio(&self) -> f64 {
        let price = self.sol_price_usd();
        if price <= 0.0 {
            return 0.5;
        }
        let sol_usd =
            lamports_to_sol(self.inner.sol_on_chain_lamports.load(Ordering::SeqCst)) * price;
        let usdc_usd = atoms_to_usdc(self.inner.usdc_on_chain_atoms.load(Ordering::SeqCst));
        let total = sol_usd + usdc_usd;
        if total <= 0.0 { 0.5 } else { sol_usd / total }
    }

    pub fn allows_direction(&self, dir: ArbDirection) -> bool {
        let r = self.ratio();
        match dir {
            ArbDirection::BuyOnDex => r < self.inner.hard_cap,
            ArbDirection::SellOnDex => r > (1.0 - self.inner.hard_cap),
        }
    }

    pub fn profit_multiplier(&self, dir: ArbDirection) -> f64 {
        let r = self.ratio();
        let in_preferred = r >= self.inner.preferred_low && r <= self.inner.preferred_high;
        if in_preferred {
            return 1.0;
        }
        let worsens = match dir {
            ArbDirection::BuyOnDex => r > self.inner.preferred_high,
            ArbDirection::SellOnDex => r < self.inner.preferred_low,
        };
        if worsens { self.inner.skewed_multiplier } else { 1.0 }
    }

    pub fn reserve(&self, dir: ArbDirection, input_amount: u64, _output_amount: u64) {
        match dir {
            ArbDirection::BuyOnDex => {
                self.inner.usdc_reserved_atoms.fetch_add(input_amount, Ordering::SeqCst);
            }
            ArbDirection::SellOnDex => {
                self.inner.sol_reserved_lamports.fetch_add(input_amount, Ordering::SeqCst);
            }
        }
    }

    pub fn commit(&self, dir: ArbDirection, input_amount: u64, output_amount: u64) {
        match dir {
            ArbDirection::BuyOnDex => {
                self.inner.usdc_on_chain_atoms.fetch_sub(input_amount, Ordering::SeqCst);
                self.inner.usdc_reserved_atoms.fetch_sub(input_amount, Ordering::SeqCst);
                self.inner.sol_on_chain_lamports.fetch_add(output_amount, Ordering::SeqCst);
            }
            ArbDirection::SellOnDex => {
                self.inner.sol_on_chain_lamports.fetch_sub(input_amount, Ordering::SeqCst);
                self.inner.sol_reserved_lamports.fetch_sub(input_amount, Ordering::SeqCst);
                self.inner.usdc_on_chain_atoms.fetch_add(output_amount, Ordering::SeqCst);
            }
        }
    }

    pub fn release(&self, dir: ArbDirection, input_amount: u64, _output_amount: u64) {
        match dir {
            ArbDirection::BuyOnDex => {
                self.inner.usdc_reserved_atoms.fetch_sub(input_amount, Ordering::SeqCst);
            }
            ArbDirection::SellOnDex => {
                self.inner.sol_reserved_lamports.fetch_sub(input_amount, Ordering::SeqCst);
            }
        }
    }
}
