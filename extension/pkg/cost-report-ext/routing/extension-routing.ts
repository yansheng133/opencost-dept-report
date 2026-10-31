import Report from '../pages/Report.vue';
import Periods from '../pages/Periods.vue';
import {
  PRODUCT_NAME, BLANK_CLUSTER, REPORT_PAGE, REPORT_ROUTE_NAME, PERIODS_PAGE, PERIODS_ROUTE_NAME
} from '../config/product';

const meta = {
  product: PRODUCT_NAME,
  cluster: BLANK_CLUSTER
};

const routes = [
  {
    name:      REPORT_ROUTE_NAME,
    path:      `/${ PRODUCT_NAME }/c/:cluster/${ REPORT_PAGE }`,
    component: Report,
    meta,
  },
  {
    name:      PERIODS_ROUTE_NAME,
    path:      `/${ PRODUCT_NAME }/c/:cluster/${ PERIODS_PAGE }`,
    component: Periods,
    meta,
  },
];

export default routes;
