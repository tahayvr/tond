/** Build the document title: most specific part first, "Tond" last. */
export function formatTitle(title?: string | null, section?: string | null): string {
	return [section, title, 'Tond'].filter((part): part is string => Boolean(part?.trim())).join(' · ');
}
