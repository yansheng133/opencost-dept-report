import { IPlugin } from '@shell/core/types';
import {
  PRODUCT_NAME, BLANK_CLUSTER, REPORT_PAGE, REPORT_ROUTE_NAME, PERIODS_PAGE, PERIODS_ROUTE_NAME
} from './config/product';

// Top-level product, following
// https://extensions.rancher.io/extensions/next/usecases/top-level-product
// The side-nav label comes from the `product.cost-report` translation key.
// Which cluster's backend is shown is chosen on the pages themselves.
export function init($plugin: IPlugin, store: any) {
  const { product, virtualType, basicType } = $plugin.DSL(store, PRODUCT_NAME);
  const to = (name: string) => ({ name, params: { product: PRODUCT_NAME, cluster: BLANK_CLUSTER } });

  product({
    svg:     require('./assets/icon.svg'),
    inStore: 'management',
    weight:  90,
    to:      to(REPORT_ROUTE_NAME),
  });

  // The report first in every language (Rancher sorts by label otherwise).
  virtualType({
    labelKey: 'costReport.nav.report',
    name:     REPORT_PAGE,
    weight:   2,
    route:    to(REPORT_ROUTE_NAME),
  });

  virtualType({
    labelKey: 'costReport.nav.periods',
    name:     PERIODS_PAGE,
    weight:   1,
    route:    to(PERIODS_ROUTE_NAME),
  });

  basicType([REPORT_PAGE, PERIODS_PAGE]);
}
