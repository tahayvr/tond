<script lang="ts">
	import type { EnvVarEntry } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import Trash2 from '~icons/ri/delete-bin-line';
	import Pencil from '~icons/ri/pencil-line';
	import Eye from '~icons/ri/eye-line';
	import EyeOff from '~icons/ri/eye-off-line';
	import Copy from '~icons/ri/file-copy-line';
	import Check from '~icons/ri/check-line';

	interface Props {
		entries: EnvVarEntry[];
		/** Plaintext values the user explicitly revealed, keyed by variable name */
		revealed: ReadonlyMap<string, string>;
		/** Key currently being revealed/copied/saved (disables its buttons) */
		busyKey: string | null;
		copiedKey: string | null;
		/** Short noun used in button labels, e.g. "variable" or "override" */
		noun: string;
		/** Show reveal and copy (admins only; the server refuses others) */
		canReveal?: boolean;
		/** When set, shows an "Exclude from previews" checkbox column */
		excluded?: ReadonlySet<string>;
		onToggleExclude?: (key: string, exclude: boolean) => void;
		onToggleReveal: (key: string) => void;
		onCopy: (key: string) => void;
		onEdit: (key: string) => void;
		onDelete: (key: string) => void;
	}

	let {
		entries,
		revealed,
		busyKey,
		copiedKey,
		noun,
		canReveal = true,
		excluded,
		onToggleExclude,
		onToggleReveal,
		onCopy,
		onEdit,
		onDelete
	}: Props = $props();
</script>

<div class="overflow-hidden rounded border border-border">
	<table class="w-full text-sm">
		<thead>
			<tr class="border-b border-border bg-muted/50">
				<th
					class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
				>
					Key
				</th>
				<th
					class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
				>
					Value
				</th>
				{#if excluded}
					<th
						class="w-24 px-3 py-2 text-center font-mono text-xs font-medium text-muted-foreground uppercase"
					>
						Exclude
					</th>
				{/if}
				<th class="w-32 px-3 py-2"><span class="sr-only">Actions</span></th>
			</tr>
		</thead>
		<tbody>
			{#each entries as entry, i (entry.key)}
				{@const value = revealed.get(entry.key)}
				{@const isRevealed = value !== undefined}
				{@const busy = busyKey === entry.key}
				<tr class={i < entries.length - 1 ? 'border-b border-border' : ''}>
					<td class="px-3 py-2 font-mono text-xs font-medium break-all">{entry.key}</td>
					<td class="max-w-0 truncate px-3 py-2 font-mono text-xs text-muted-foreground">
						{#if !entry.masked}
							<span class="italic">empty</span>
						{:else if isRevealed}
							<span class="text-foreground" title={value}>{value}</span>
						{:else}
							<span class="tracking-widest" aria-label="hidden value">{entry.masked}</span>
						{/if}
					</td>
					{#if excluded && onToggleExclude}
						<td class="px-3 py-2 text-center">
							<input
								type="checkbox"
								checked={excluded.has(entry.key)}
								disabled={busy}
								onchange={(e) => onToggleExclude(entry.key, e.currentTarget.checked)}
								class="h-4 w-4 cursor-pointer accent-primary"
								aria-label={`Exclude ${entry.key} from previews`}
							/>
						</td>
					{/if}
					<td class="px-3 py-2">
						<div class="flex items-center justify-end gap-1">
							{#if entry.masked && canReveal}
								<Button
									variant="ghost"
									size="icon"
									class="h-7 w-7 text-muted-foreground hover:text-foreground"
									aria-label={isRevealed ? `Hide ${entry.key}` : `Reveal ${entry.key}`}
									disabled={busy}
									onclick={() => onToggleReveal(entry.key)}
								>
									{#if isRevealed}
										<EyeOff class="h-3.5 w-3.5" />
									{:else}
										<Eye class="h-3.5 w-3.5" />
									{/if}
								</Button>
								<Button
									variant="ghost"
									size="icon"
									class="h-7 w-7 text-muted-foreground hover:text-foreground"
									aria-label={`Copy ${entry.key}`}
									disabled={busy}
									onclick={() => onCopy(entry.key)}
								>
									{#if copiedKey === entry.key}
										<Check class="h-3.5 w-3.5 text-success" />
									{:else}
										<Copy class="h-3.5 w-3.5" />
									{/if}
								</Button>
							{/if}
							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7 text-muted-foreground hover:text-foreground"
								aria-label={`Edit ${noun} ${entry.key}`}
								disabled={busy}
								onclick={() => onEdit(entry.key)}
							>
								<Pencil class="h-3.5 w-3.5" />
							</Button>
							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7 text-muted-foreground hover:text-destructive"
								aria-label={`Delete ${noun} ${entry.key}`}
								disabled={busy}
								onclick={() => onDelete(entry.key)}
							>
								<Trash2 class="h-3.5 w-3.5" />
							</Button>
						</div>
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
