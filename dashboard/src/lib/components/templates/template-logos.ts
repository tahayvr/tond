// Logos for template manifests' `logo` (Simple Icons slugs), bundled per icon
// by unplugin-icons. A template whose slug isn't here shows its monogram.

import type { Component } from 'svelte';
import ActualBudget from '~icons/simple-icons/actualbudget';
import Appsmith from '~icons/simple-icons/appsmith';
import Coder from '~icons/simple-icons/coder';
import Directus from '~icons/simple-icons/directus';
import Excalidraw from '~icons/simple-icons/excalidraw';
import Forgejo from '~icons/simple-icons/forgejo';
import FreshRss from '~icons/simple-icons/freshrss';
import Ghost from '~icons/simple-icons/ghost';
import Gitea from '~icons/simple-icons/gitea';
import Grafana from '~icons/simple-icons/grafana';
import Immich from '~icons/simple-icons/immich';
import Listmonk from '~icons/simple-icons/listmonk';
import Mattermost from '~icons/simple-icons/mattermost';
import Mealie from '~icons/simple-icons/mealie';
import Metabase from '~icons/simple-icons/metabase';
import N8n from '~icons/simple-icons/n8n';
import Nextcloud from '~icons/simple-icons/nextcloud';
import Ntfy from '~icons/simple-icons/ntfy';
import PaperlessNgx from '~icons/simple-icons/paperlessngx';
import Plausible from '~icons/simple-icons/plausibleanalytics';
import PocketBase from '~icons/simple-icons/pocketbase';
import Prometheus from '~icons/simple-icons/prometheus';
import Twenty from '~icons/simple-icons/twenty';
import Umami from '~icons/simple-icons/umami';
import UptimeKuma from '~icons/simple-icons/uptimekuma';
import Vaultwarden from '~icons/simple-icons/vaultwarden';
import WikiJs from '~icons/simple-icons/wikidotjs';

export const TEMPLATE_LOGOS: Record<string, Component> = {
	actualbudget: ActualBudget,
	appsmith: Appsmith,
	coder: Coder,
	directus: Directus,
	excalidraw: Excalidraw,
	forgejo: Forgejo,
	freshrss: FreshRss,
	ghost: Ghost,
	gitea: Gitea,
	grafana: Grafana,
	immich: Immich,
	listmonk: Listmonk,
	mattermost: Mattermost,
	mealie: Mealie,
	metabase: Metabase,
	n8n: N8n,
	nextcloud: Nextcloud,
	ntfy: Ntfy,
	paperlessngx: PaperlessNgx,
	plausibleanalytics: Plausible,
	pocketbase: PocketBase,
	prometheus: Prometheus,
	twenty: Twenty,
	umami: Umami,
	uptimekuma: UptimeKuma,
	vaultwarden: Vaultwarden,
	wikidotjs: WikiJs
};

/** Up to two letters for templates without a logo ("IT Tools" → "IT"). */
export function monogram(name: string): string {
	const words = name.split(/[\s-]+/).filter(Boolean);
	if (words.length >= 2) return (words[0][0] + words[1][0]).toUpperCase();
	return name.slice(0, 2).replace(/^./, (c) => c.toUpperCase());
}
