// The Pricing seam: "remote but owned". Thin adapter over the Tauri IPC fns in
// src/api.ts, mirroring ledger.ts so a page depends on this port instead of
// @tauri-apps directly (lets tests swap in pricing.fake.ts). No logic here.
import { listen } from '@tauri-apps/api/event';
import { modelPricing, setModelOverride, deleteModelOverride } from '../api';
import type { ModelPricing, RatesPerTok } from '../types';

export interface PricingPort {
  list(): Promise<ModelPricing[]>;
  setOverride(model: string, rates: RatesPerTok): Promise<void>;
  deleteOverride(model: string): Promise<void>;
  onPricesRebuilt(cb: () => void): () => void; // subscribe, returns unsubscribe
}

export const tauriPricing: PricingPort = {
  list: modelPricing,
  setOverride: setModelOverride,
  deleteOverride: deleteModelOverride,
  onPricesRebuilt(cb) {
    // listen() is async; the unsubscribe resolves later, so teardown must await it.
    const un = listen('prices-rebuilt', () => cb());
    return () => {
      un.then((f) => f());
    };
  },
};
