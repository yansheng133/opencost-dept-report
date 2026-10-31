// What every section of a page needs besides the data: translation,
// department names and colors, the label keys and the currency. Provided
// once by the page, injected by the sections.
import {
  inject, provide, InjectionKey, ComputedRef, computed
} from 'vue';
import { deptIndex, DeptIndex } from '../utils/scope';
import type { BackendConfig } from '../utils/types';
import type { Translate } from './useText';

export interface ReportContext {
  t: Translate;
  depts: DeptIndex;
  // The department and environment label keys (cost-center, env).
  label: string;
  envLabel: string;
  environments: string[];
  currency: string;
}

const KEY: InjectionKey<ComputedRef<ReportContext>> = Symbol('cost-report-context');

export function provideReportContext(t: Translate, config: ComputedRef<BackendConfig | null>): ComputedRef<ReportContext> {
  const ctx = computed<ReportContext>(() => {
    const c = config.value;

    return {
      t,
      depts:        deptIndex(c?.departments || []),
      label:        c?.labels?.department || 'cost-center',
      envLabel:     c?.labels?.environment || 'env',
      environments: c?.labels?.environments?.length ? c.labels.environments : ['prod', 'dev'],
      currency:     c?.currency || t('costReport.common.currencyFallback'),
    };
  });

  provide(KEY, ctx);

  return ctx;
}

export function useReportContext(): ComputedRef<ReportContext> {
  const ctx = inject(KEY);

  if (!ctx) {
    throw new Error('useReportContext outside a cost-report page');
  }

  return ctx;
}
