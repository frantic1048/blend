interface Heading {
  slug: string;
  text: string;
}

function attribute(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// Append a separate permalink so headings can keep their existing inline links.
// Use the Markdown renderer's IDs to preserve table-of-contents and shared URLs.
export function headingLinks(html: string, headings: Heading[] = []): string {
  const labels = new Map(headings.map(({ slug, text }) => [attribute(slug), text]));
  return html.replace(
    /(<h([2-6])\b[^>]*\bid="([^"]+)"[^>]*>)([\s\S]*?)(<\/h\2>)/g,
    (_match, open, _depth, id: string, content, close) => {
      const label = attribute(`Link to section: ${labels.get(id) ?? id}`);
      return `${open}${content}<a class="heading-link" href="#${id}" aria-label="${label}"><span aria-hidden="true">#</span></a>${close}`;
    },
  );
}
