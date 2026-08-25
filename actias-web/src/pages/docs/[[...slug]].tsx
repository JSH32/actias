/* eslint-disable react/jsx-props-no-spreading */
import * as React from 'react';
import type { GetStaticPaths, GetStaticProps } from 'next';
import Link from 'next/link';
import { MDXRemote, MDXRemoteSerializeResult } from 'next-mdx-remote';
import { serialize } from 'next-mdx-remote/serialize';
import { NextSeo } from 'next-seo';
import type { MDXComponents } from 'mdx/types';
import rehypeSlug from 'rehype-slug';

import {
  DocGroup,
  DocHeading,
  DocMeta,
  SearchEntry,
  getAllDocs,
  getDoc,
  getNav,
  getReadingOrder,
  getSearchIndex,
} from '@/helpers/docs';
import { DocSidebar } from '@/components/docs/DocSidebar';
import classes from './docs.module.css';

interface Props {
  source: MDXRemoteSerializeResult;
  meta: DocMeta;
  headings: DocHeading[];
  nav: DocGroup[];
  index: SearchEntry[];
  prev: { slug: string; title: string } | null;
  next: { slug: string; title: string } | null;
}

/** Prose renders in the reading serif; code and tables stay UI type. */
const components: MDXComponents = {
  h2: (props) => <h2 className={classes.h2} {...props} />,
  h3: (props) => <h3 className={classes.h3} {...props} />,
  p: (props) => <p className={classes.p} {...props} />,
  ul: (props) => <ul className={classes.ul} {...props} />,
  ol: (props) => <ol className={classes.ul} {...props} />,
  li: (props) => <li className={classes.li} {...props} />,
  a: (props) => <a className={classes.a} {...props} />,
  hr: () => <hr className={classes.hr} />,
  blockquote: (props) => <blockquote className={classes.quote} {...props} />,
  table: (props) => (
    <div className={classes.tableWrap}>
      <table className={classes.table} {...props} />
    </div>
  ),
  pre: (props) => <pre className={classes.pre} {...props} />,
  code: (props: { className?: string }) =>
    props.className ? (
      <code {...props} />
    ) : (
      <code className={classes.inlineCode} {...props} />
    ),
};

export default function DocPage({
  source,
  meta,
  headings,
  nav,
  index,
  prev,
  next,
}: Props) {
  return (
    <>
      <NextSeo title={`${meta.title} · Actias docs`} description={meta.lead} />
      <div className={classes.frame}>
        <DocSidebar nav={nav} index={index} active={meta.slug} />

        <main className={classes.main}>
          <article className={classes.article}>
            <h1 className={classes.h1}>{meta.title}</h1>
            {meta.lead && <p className={classes.lead}>{meta.lead}</p>}
            <MDXRemote {...source} components={components} />

            <nav className={classes.pager}>
              {prev ? (
                <Link href={`/docs/${prev.slug}`} className={classes.pagerLink}>
                  <span className={classes.pagerDir}>Previous</span>
                  <span className={classes.pagerTitle}>{prev.title}</span>
                </Link>
              ) : (
                <span />
              )}
              {next && (
                <Link
                  href={`/docs/${next.slug}`}
                  className={`${classes.pagerLink} ${classes.pagerNext}`}
                >
                  <span className={classes.pagerDir}>Next</span>
                  <span className={classes.pagerTitle}>{next.title}</span>
                </Link>
              )}
            </nav>
          </article>

          {headings.length > 1 && (
            <aside className={classes.toc}>
              <div className={classes.tocLabel}>On this page</div>
              {headings.map((heading) => (
                <a
                  key={heading.id}
                  href={`#${heading.id}`}
                  className={
                    heading.depth === 3 ? classes.tocSub : classes.tocItem
                  }
                >
                  {heading.text}
                </a>
              ))}
            </aside>
          )}
        </main>
      </div>
    </>
  );
}

export const getStaticPaths: GetStaticPaths = async () => ({
  paths: getAllDocs().map((doc) => ({ params: { slug: doc.slug.split('/') } })),
  fallback: false,
});

export const getStaticProps: GetStaticProps = async ({ params }) => {
  const order = getReadingOrder();
  // Bare /docs opens the first page in reading order.
  const slug = Array.isArray(params?.slug)
    ? (params?.slug as string[]).join('/')
    : order[0].slug;

  const { content, headings, meta } = getDoc(slug);
  const source = await serialize(content, {
    mdxOptions: { rehypePlugins: [rehypeSlug] },
  });

  const at = order.findIndex((doc) => doc.slug === slug);
  const step = (index: number) =>
    index >= 0 && index < order.length
      ? { slug: order[index].slug, title: order[index].title }
      : null;

  return {
    props: {
      source,
      meta,
      headings,
      nav: getNav(),
      index: getSearchIndex(),
      prev: step(at - 1),
      next: step(at + 1),
    },
  };
};
