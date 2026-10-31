<script lang="ts">
// 資料可信度: how the figures were obtained and how complete they are.
// Coverage is shown to everyone; reconciliation, fill notes and monitoring
// targets only with full scope.
import { defineComponent, PropType, computed, ref } from 'vue';
import CrSection from '../CrSection.vue';
import Help from '../Help.vue';
import MoreRow from '../MoreRow.vue';
import { useReportContext } from '../../composables/useReportContext';
import { visibleParts } from '../../utils/scope';
import { gapText, fillNoteText } from '../../utils/describe';
import { capRows, pctValue } from '../../utils/format';
import { TABLE_LIMITS } from '../../config/backend';
import type { Report } from '../../utils/types';
import { fmtMoney } from '../../utils/money';

export default defineComponent({
  name: 'CostReportTrust',

  components: {
    CrSection, Help, MoreRow
  },

  props: { report: { type: Object as PropType<Report>, required: true } },

  setup(props) {
    const ctx = useReportContext();
    const open = ref(false);
    const parts = computed(() => visibleParts(props.report));
    const rows = computed(() => {
      const { t } = ctx.value;
      const k = 'costReport.trust';
      const cov = props.report.coverage;
      const b = props.report.basis || {
        estimatedTotal: 0, estimatedPct: 0, fill: null
      };
      const f = parts.value.fill ? b.fill : null;
      const dash = (v: unknown) => (v === null || v === undefined ? '—' : String(v));
      const out: { item: string; value: string; note: string }[] = [];

      if (!cov) {
        // No coverage at all: the period has no data to measure, or
        // Prometheus is not configured. Not "unreachable" with no reason.
        out.push({
          item: t(`${ k }.effective`), value: t(`${ k }.effectiveUnknown`), note: props.report.total ? t(`${ k }.effectiveNotConfigured`) : t(`${ k }.effectiveNoData`)
        });
      } else if (cov.available === false) {
        out.push({
          item: t(`${ k }.effective`), value: t(`${ k }.effectiveUnknown`), note: cov.reason ? t(`${ k }.effectiveNoProm`, { reason: cov.reason }) : t(`${ k }.effectiveNoPromNoReason`)
        });
      } else {
        out.push({
          item: t(`${ k }.effective`), value: pctValue(cov.pct), note: t(`${ k }.effectiveNote`, { scrape: dash(cov.scrapePct), window: pctValue(cov.windowPct) })
        });
        out.push({
          item: t(`${ k }.requested`), value: t(`${ k }.requestedValue`, { measured: dash(cov.measuredMinutes), requested: dash(cov.requestedMinutes) }), note: t(`${ k }.requestedNote`)
        });
        out.push({
          item: t(`${ k }.gaps`), value: t(`${ k }.gapsValue`, { minutes: cov.gapMinutes || 0 }), note: cov.measuredStep ? t(`${ k }.gapsNoteMeasured`, { seconds: dash(cov.stepSeconds) }) : t(`${ k }.gapsNoteGuessed`)
        });
        const gaps = cov.gaps || [];

        out.push({
          item: t(`${ k }.gapList`), value: gaps.length ? t(`${ k }.gapListValue`, { count: gaps.length }) : t(`${ k }.gapListNone`), note: gaps.map((g) => gapText(t, g)).join('; ') || t(`${ k }.gapListNoneNote`)
        });
      }
      out.push({
        item:  t(`${ k }.estimated`),
        value: pctValue(b.estimatedPct || 0),
        note:  f ? t(`${ k }.estimatedFill`, { amount: fmtMoney(b.estimatedTotal) }) + (f.snapshotCoveragePct !== null && f.snapshotCoveragePct !== undefined ? t(`${ k }.estimatedFillCoverage`, { pct: f.snapshotCoveragePct }) : '') : t(`${ k }.estimatedNone`),
      });
      if (parts.value.reconciliation) {
        const r = props.report.reconciliation!;

        out.push({
          item:  t(`${ k }.recon`),
          value: pctValue(r.driftPct),
          // A zero or negative asset total is OpenCost not having priced the
          // nodes yet, not a cost to reconcile against (reason from the backend).
          // The asset components are listed next to the total they add up
          // to; when the spans differ, that total is then scaled.
          note:  r.reason === 'assets_not_positive' ? t(`${ k }.reconNotPositive`, { assets: fmtMoney(r.assets) }) : r.assets ? t(r.scaled ? `${ k }.reconNoteScaled` : `${ k }.reconNote`, {
            allocated:     fmtMoney(r.allocated),
            assets:        fmtMoney(r.assets),
            measured:      fmtMoney(r.scaled ? r.assetsMeasured : r.assets),
            byType:        Object.entries(r.byType || {}).map(([x, v]) => `${ x } ${ fmtMoney(v) }`).join(t(`${ k }.listSep`)),
            assetsMinutes: r.assetsMinutes,
            minutes:       r.allocatedMinutes,
          }) : t(`${ k }.reconUnknown`),
        });
      }
      if (f && f.notes?.length) {
        out.push({
          item: t(`${ k }.fillNotes`), value: t(`${ k }.fillNotesValue`, { count: f.notes.length }), note: f.notes.map((n) => fillNoteText(t, n)).join('; ')
        });
      }

      return out;
    });
    const targets = computed(() => props.report.targets || []);
    const shownTargets = computed(() => capRows(targets.value, TABLE_LIMITS.targets, open.value).shown);

    const summary = computed(() => {
      const c = props.report.coverage;

      return c && c.available !== false ? ctx.value.t('costReport.trust.summary', { pct: pctValue(c.pct) }) : ctx.value.t('costReport.trust.summaryUnknown');
    });

    return {
      ctx, parts, rows, targets, shownTargets, open, summary, limit: TABLE_LIMITS.targets
    };
  },
});
</script>

<template>
  <CrSection
    id="trust"
    :title="ctx.t('costReport.trust.title')"
    :description="ctx.t('costReport.trust.description')"
    :summary="summary"
    cluster-wide
  >
    <div class="cr-table-wrap">
      <table class="sortable-table cr-table cr-trust-table">
        <thead>
          <tr>
            <th>{{ ctx.t('costReport.trust.col.item') }}</th>
            <th>{{ ctx.t('costReport.trust.col.value') }}</th>
            <th>{{ ctx.t('costReport.trust.col.note') }}</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="r in rows"
            :key="r.item"
          >
            <td>{{ r.item }}</td>
            <td class="cr-num">
              {{ r.value }}
            </td>
            <td class="text-muted cr-small">
              {{ r.note }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <template v-if="parts.targets">
      <h3 class="mt-20">
        {{ ctx.t('costReport.trust.targetsTitle') }}
      </h3>
      <p class="text-muted cr-small">
        {{ ctx.t('costReport.trust.targetsDescription') }}
      </p>
      <div class="cr-table-wrap">
        <table class="sortable-table cr-table">
          <thead>
            <tr>
              <th>
                <Help
                  :text="ctx.t('costReport.trust.col2.job')"
                  :tip="ctx.t('costReport.help.job')"
                />
              </th>
              <th>
                <Help
                  :text="ctx.t('costReport.trust.col2.expected')"
                  :tip="ctx.t('costReport.help.expected')"
                />
              </th>
              <th>
                <Help
                  :text="ctx.t('costReport.trust.col2.worst')"
                  :tip="ctx.t('costReport.help.worst')"
                />
              </th>
              <th>
                <Help
                  :text="ctx.t('costReport.trust.col2.degraded')"
                  :tip="ctx.t('costReport.help.degraded')"
                />
              </th>
              <th>{{ ctx.t('costReport.trust.col2.status') }}</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="tg in shownTargets"
              :key="tg.job"
            >
              <td>{{ tg.job }}</td>
              <td class="cr-num">
                {{ tg.expected }}
              </td>
              <td class="cr-num">
                {{ tg.worstHealthy }}
              </td>
              <td class="cr-num">
                {{ tg.degradedPct }}%
              </td>
              <td>
                <span v-if="tg.status === 'ok'">{{ ctx.t('costReport.trust.targetOk') }}</span>
                <b
                  v-else
                  class="cr-warn"
                >{{ ctx.t('costReport.trust.targetDegraded') }}</b>
              </td>
            </tr>
            <tr v-if="!targets.length">
              <td
                colspan="5"
                class="text-muted"
              >
                {{ ctx.t('costReport.trust.targetsNone') }}
              </td>
            </tr>
            <MoreRow
              :total="targets.length"
              :limit="limit"
              :open="open"
              :cols="5"
              @toggle="open = !open"
            />
          </tbody>
        </table>
      </div>
    </template>
  </CrSection>
</template>
