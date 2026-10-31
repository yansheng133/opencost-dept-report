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
import { capRows, n4, pctValue } from '../../utils/format';
import { TABLE_LIMITS } from '../../config/backend';
import type { Report } from '../../utils/types';

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

      if (!cov || cov.available === false) {
        out.push({
          item: t(`${ k }.effective`), value: t(`${ k }.effectiveUnknown`), note: t(`${ k }.effectiveNoProm`, { reason: cov?.reason || '' })
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
        note:  f ? t(`${ k }.estimatedFill`, { amount: n4(b.estimatedTotal) }) + (f.snapshotCoveragePct !== null && f.snapshotCoveragePct !== undefined ? t(`${ k }.estimatedFillCoverage`, { pct: f.snapshotCoveragePct }) : '') : t(`${ k }.estimatedNone`),
      });
      if (parts.value.reconciliation) {
        const r = props.report.reconciliation!;

        out.push({
          item:  t(`${ k }.recon`),
          value: pctValue(r.driftPct),
          // A zero or negative asset total is OpenCost not having priced the
          // nodes yet, not a cost to reconcile against (reason from the backend).
          note:  r.reason === 'assets_not_positive' ? t(`${ k }.reconNotPositive`, { assets: n4(r.assets) }) : r.assets ? t(`${ k }.reconNote`, {
            allocated: n4(r.allocated), assets: n4(r.assets), byType: Object.entries(r.byType || {}).map(([x, v]) => `${ x } ${ n4(v) }`).join(', ')
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

    return {
      ctx, parts, rows, targets, shownTargets, open, limit: TABLE_LIMITS.targets
    };
  },
});
</script>

<template>
  <CrSection
    id="trust"
    :title="ctx.t('costReport.trust.title')"
    :description="ctx.t('costReport.trust.description')"
  >
    <div class="cr-table-wrap">
      <table class="sortable-table cr-table">
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
