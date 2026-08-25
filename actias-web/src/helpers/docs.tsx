import fs from 'fs';
import { sync } from 'glob';
import matter from 'gray-matter';
import normalize from 'normalize-path';
import path from 'path';

/** Frontmatter every docs page carries. */
export interface DocFrontmatter {
  title: string;
  /** One sentence under the title, and the search result's subtitle. */
  lead: string;
  /** Sort key inside the folder. */
  order: number;
  /** Shorter label when the title is too long for the sidebar. */
  navLabel?: string | null;
}

export interface DocMeta extends DocFrontmatter {
  /** Path under /docs, e.g. "runtime/objects". */
  slug: string;
  /** Folder chain above the page, outermost first. */
  section: string[];
}

/** One sidebar node: a page, its children, and the folder it opens. */
export interface DocNode {
  label: string;
  slug?: string;
  children: DocNode[];
}

export interface DocGroup {
  title: string;
  order: number;
  items: DocNode[];
}

/** A page's headings, for the on-page contents rail. */
export interface DocHeading {
  id: string;
  text: string;
  depth: number;
}

/** What the client-side search filters over. */
export interface SearchEntry {
  slug: string;
  title: string;
  lead: string;
  section: string;
  body: string;
}

const DOCS_PATH = path.join(process.cwd(), 'src/content/docs');

/** A folder's own title and position, from its group.json. */
interface GroupMeta {
  title: string;
  order: number;
}

function readGroup(dir: string): GroupMeta {
  const file = path.join(dir, 'group.json');
  if (fs.existsSync(file)) {
    return JSON.parse(fs.readFileSync(file, 'utf8')) as GroupMeta;
  }
  return { title: path.basename(dir), order: 99 };
}

function slugify(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^\w\s-]/g, '')
    .trim()
    .replace(/\s+/g, '-');
}

/** Every page, in file order. */
export function getAllDocs(): DocMeta[] {
  const paths = sync(normalize(`${DOCS_PATH}/**/*.mdx`));

  return paths.map((file) => {
    const relative = path.relative(DOCS_PATH, file).replace(/\.mdx$/, '');
    const parts = relative.split('/');
    const source = fs.readFileSync(file);
    const { data } = matter(source);

    return {
      slug: relative,
      section: parts.slice(0, -1),
      title: data.title ?? parts[parts.length - 1],
      lead: data.lead ?? '',
      order: data.order ?? 99,
      navLabel: data.navLabel ?? null,
    };
  });
}

/** One page's content, frontmatter and headings. */
export function getDoc(slug: string) {
  const file = path.join(DOCS_PATH, `${slug}.mdx`);
  const { content, data } = matter(fs.readFileSync(file));

  // The rail lists what the reader can jump to: h2 and h3 only.
  const headings: DocHeading[] = [];
  let fenced = false;
  content.split('\n').forEach((line) => {
    if (line.startsWith('```')) fenced = !fenced;
    if (fenced) return;
    const match = /^(##|###)\s+(.*)$/.exec(line);
    if (match) {
      const text = match[2].replace(/`/g, '').trim();
      headings.push({ id: slugify(text), text, depth: match[1].length });
    }
  });

  return {
    content,
    headings,
    meta: {
      slug,
      section: slug.split('/').slice(0, -1),
      title: data.title ?? slug,
      lead: data.lead ?? '',
      order: data.order ?? 99,
      navLabel: data.navLabel ?? null,
    } as DocMeta,
  };
}

/**
 * The sidebar tree. Top-level folders are groups; a page named after a
 * sibling folder becomes that folder's parent row, so `objects.mdx`
 * beside `objects/` renders as one branch.
 */
export function getNav(): DocGroup[] {
  const docs = getAllDocs();
  const groups = new Map<string, DocGroup>();

  docs.forEach((doc) => {
    const [top] = doc.section;
    if (!top) return;
    if (!groups.has(top)) {
      const meta = readGroup(path.join(DOCS_PATH, top));
      groups.set(top, { title: meta.title, order: meta.order, items: [] });
    }
  });

  groups.forEach((group, key) => {
    const inGroup = docs.filter((doc) => doc.section[0] === key);
    const roots = inGroup
      .filter((doc) => doc.section.length === 1)
      .sort((a, b) => a.order - b.order);

    group.items = roots.map((doc) => {
      const name = doc.slug.split('/').pop() as string;
      const children = inGroup
        .filter((child) => child.section[1] === name)
        .sort((a, b) => a.order - b.order)
        .map((child) => ({
          label: child.navLabel ?? child.title,
          slug: child.slug,
          children: [],
        }));

      return { label: doc.navLabel ?? doc.title, slug: doc.slug, children };
    });
  });

  return Array.from(groups.values()).sort((a, b) => a.order - b.order);
}

/** Reading order for the prev/next footer: the sidebar, flattened. */
export function getReadingOrder(): DocMeta[] {
  const docs = new Map(getAllDocs().map((doc) => [doc.slug, doc]));
  const flat: DocMeta[] = [];

  getNav().forEach((group) => {
    group.items.forEach((item) => {
      if (item.slug && docs.has(item.slug))
        flat.push(docs.get(item.slug) as DocMeta);
      item.children.forEach((child) => {
        if (child.slug && docs.has(child.slug)) {
          flat.push(docs.get(child.slug) as DocMeta);
        }
      });
    });
  });

  return flat;
}

/** The search index, built once at build time and shipped to the page. */
export function getSearchIndex(): SearchEntry[] {
  const nav = getNav();
  const sectionOf = new Map<string, string>();
  nav.forEach((group) =>
    group.items.forEach((item) => {
      if (item.slug) sectionOf.set(item.slug, group.title);
      item.children.forEach((child) => {
        if (child.slug) sectionOf.set(child.slug, group.title);
      });
    }),
  );

  return getAllDocs().map((doc) => {
    const { content } = matter(
      fs.readFileSync(path.join(DOCS_PATH, `${doc.slug}.mdx`)),
    );
    return {
      slug: doc.slug,
      title: doc.title,
      lead: doc.lead,
      section: sectionOf.get(doc.slug) ?? '',
      // Prose only: fences and markdown punctuation add noise, not matches.
      body: content
        .replace(/```[\s\S]*?```/g, ' ')
        .replace(/[#*`_>|-]/g, ' ')
        .replace(/\s+/g, ' ')
        .slice(0, 4000),
    };
  });
}
