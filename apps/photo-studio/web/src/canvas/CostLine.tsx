import { useCost, useMe } from '../api/queries';
import { formatCredits } from '../lib/util';
import { Spinner } from '../components/ui';

export function CostLine({ model, mode, params }: { model?: string; mode?: string; params: Record<string, unknown> }) {
  const me = useMe();
  const cost = useCost(model, mode, params);
  const balance = me.data?.openart.credits;
  const credits = cost.data?.credits;
  const count = Number(params.resultType === 'series' ? params.seriesAmount : params.imageCount);
  const unit = credits !== undefined && Number.isFinite(count) && count > 1 ? credits / count : undefined;
  const over = cost.data ? !cost.data.affordable : false;
  return (
    <div className={`cost ${over ? 'is-over' : ''}`} aria-live="polite">
      <span className="cost-main mono">
        {cost.isFetching && credits === undefined ? <Spinner size={12} /> : `${formatCredits(credits)} cr`}
        {cost.isFetching && credits !== undefined ? <span className="cost-pending" aria-hidden /> : null}
      </span>
      <span className="cost-sub mono">
        {unit !== undefined ? `${count} × ${formatCredits(unit)}` : cost.isError ? 'price unavailable' : 'per job'}
      </span>
      <span className="cost-balance mono" title="OpenArt balance">
        {over ? 'over balance · ' : 'bal '}
        {formatCredits(balance)}
      </span>
    </div>
  );
}
