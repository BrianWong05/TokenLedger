import type { ComponentType } from 'react';

export interface RootModule {
  default: ComponentType;
}

export interface RootLoaders {
  app: () => Promise<RootModule>;
  panel: () => Promise<RootModule>;
}

const productionLoaders: RootLoaders = {
  app: () => import('./App'),
  panel: () => import('./traypanel/TrayPanel'),
};

export function loadRootModule(
  isPanel: boolean,
  loaders: RootLoaders = productionLoaders,
): Promise<RootModule> {
  return isPanel ? loaders.panel() : loaders.app();
}
