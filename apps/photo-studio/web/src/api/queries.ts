import { keepPreviousData, useInfiniteQuery, useQuery, useQueryClient, QueryClient } from '@tanstack/react-query';
import { api, ApiError } from './client';
import type { JSONSchema, Model, ModelMode } from './types';
import { stableStringify, useDebounced } from '../lib/util';

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: (count, err) => {
        if (err instanceof ApiError && err.status >= 400 && err.status < 500) return false;
        return count < 2;
      },
      refetchOnWindowFocus: false,
      staleTime: 30_000,
    },
  },
});

export const qk = {
  me: ['me'] as const,
  models: ['models'] as const,
  form: (model: string, mode: string) => ['form', model, mode] as const,
  cost: (model: string, mode: string, params: string) => ['cost', model, mode, params] as const,
  assets: ['assets'] as const,
  activeJobs: ['jobs', 'active'] as const,
  workflows: ['workflows'] as const,
};

export function useMe() {
  return useQuery({ queryKey: qk.me, queryFn: api.me, staleTime: 60_000, retry: 1 });
}

export function useModels(enabled = true) {
  return useQuery({ queryKey: qk.models, queryFn: () => api.models(), staleTime: 10 * 60_000, enabled });
}

export function useModelForm(model: string | undefined, mode: string | undefined) {
  return useQuery({
    queryKey: qk.form(model ?? '', mode ?? ''),
    queryFn: () => api.form(model ?? '', mode ?? ''),
    enabled: Boolean(model && mode),
    staleTime: 60 * 60_000,
    select: (r): JSONSchema => r.schema,
  });
}

/** Live credit cost for a configuration; the request is debounced while the user edits. */
export function useCost(model: string | undefined, mode: string | undefined, params: Record<string, unknown>) {
  const key = useDebounced(stableStringify(params), 350);
  return useQuery({
    queryKey: qk.cost(model ?? '', mode ?? '', key),
    queryFn: ({ signal }) => api.cost(model ?? '', mode ?? '', JSON.parse(key) as Record<string, unknown>, signal),
    enabled: Boolean(model && mode),
    placeholderData: keepPreviousData,
    staleTime: 60_000,
  });
}

export function useAssets() {
  return useInfiniteQuery({
    queryKey: qk.assets,
    queryFn: ({ pageParam }) => api.assets(pageParam),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next,
  });
}

export function useActiveJobs(enabled: boolean) {
  return useQuery({
    queryKey: qk.activeJobs,
    queryFn: api.activeJobs,
    enabled,
    refetchInterval: (q) => ((q.state.data?.jobs.length ?? 0) > 0 ? 2000 : 8000),
  });
}

export function useWorkflows(enabled = true) {
  return useQuery({ queryKey: qk.workflows, queryFn: api.workflows, enabled, staleTime: 10_000 });
}

export function useInvalidate() {
  const qc = useQueryClient();
  return (key: readonly unknown[]) => qc.invalidateQueries({ queryKey: key });
}

/** The image modes of a model (text2image, image2image…). */
export function imageModes(m: Model | undefined): ModelMode[] {
  return m?.modes.image ?? [];
}

export function findModel(models: Model[] | undefined, id: string | undefined): Model | undefined {
  if (!models || !id) return undefined;
  return models.find((m) => m.id === id);
}
