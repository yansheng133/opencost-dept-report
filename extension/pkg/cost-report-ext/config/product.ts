// Shared names for the product, its pages and routes.
// Defined once so product.ts and routing stay in sync (as recommended in
// https://extensions.rancher.io/extensions/next/usecases/top-level-product).

export const PRODUCT_NAME = 'cost-report';

// Rancher's "blank cluster" value for top-level products. Which cluster's
// cost-report is shown is chosen on the page (utils/target.ts), not here.
export const BLANK_CLUSTER = '_';

// 部門成本: the report of one period (window or day).
export const REPORT_PAGE = 'departments';

export const REPORT_ROUTE_NAME = `${ PRODUCT_NAME }-c-cluster-${ REPORT_PAGE }`;

// 帳期: sealed days and the monthly roll-up made from them.
export const PERIODS_PAGE = 'periods';

export const PERIODS_ROUTE_NAME = `${ PRODUCT_NAME }-c-cluster-${ PERIODS_PAGE }`;

/** Route location of the report, optionally for one sealed day. */
export function reportRoute(day?: string) {
  return {
    name:   REPORT_ROUTE_NAME,
    params: { product: PRODUCT_NAME, cluster: BLANK_CLUSTER },
    query:  day ? { day } : {},
  };
}

// Locales that Rancher 2.15 / @rancher/shell 3.0.13 does not ship.
// Registering zh-hant makes it selectable in the language menu; strings
// outside this extension fall back to English.
export const EXTRA_LOCALES: { locale: string; label: string }[] = [
  { locale: 'zh-hant', label: '繁體中文' },
];
