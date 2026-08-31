/* eslint-disable react/jsx-props-no-spreading */
import React from 'react';

import type { GetStaticPaths, GetStaticProps } from 'next';
import { MDXRemote, MDXRemoteSerializeResult } from 'next-mdx-remote';
import { serialize } from 'next-mdx-remote/serialize';
import { NextSeo } from 'next-seo';
import Link from 'next/link';

import type { MDXComponents } from 'mdx/types';
import rehypeAutolinkHeadings from 'rehype-autolink-headings';
import rehypeSlug from 'rehype-slug';

import { PostMeta, getPostFromSlug, getSlugs } from '@/helpers/blog';
import { Icon } from '@/ui/icons';
import { highlightLua } from '@/components/home/lua';
import classes from './post.module.css';

interface MDXPost {
  source: MDXRemoteSerializeResult<Record<string, unknown>>;
  meta: PostMeta;
}

/** The language a fence declared, from the class remark puts on `code`. */
function fenceLanguage(node: React.ReactNode): string | null {
  if (!React.isValidElement(node)) return null;
  const className = (node.props as { className?: string }).className ?? '';
  const found = className
    .split(' ')
    .find((name) => name.startsWith('language-'));
  return found ? found.slice('language-'.length) : null;
}

function fenceSource(node: React.ReactNode): string {
  if (!React.isValidElement(node)) return '';
  const children = (node.props as { children?: unknown }).children;
  return typeof children === 'string' ? children : '';
}

const heading =
  (level: 'h2' | 'h3' | 'h4', style: string) =>
  // eslint-disable-next-line react/display-name
  (props: object) => {
    const Tag = level;
    return <Tag className={style} {...props} />;
  };

const components: MDXComponents = {
  // eslint-disable-next-line @next/next/no-img-element
  img: (props: { src?: string; alt?: string }) => (
    // eslint-disable-next-line @next/next/no-img-element
    <img style={{ maxWidth: '100%' }} src={props.src} alt={props.alt} />
  ),
  code: (props: object) => <code className={classes.inline} {...props} />,
  pre: (props: React.HTMLAttributes<HTMLPreElement>) => {
    const child = props.children;
    const language = fenceLanguage(child);
    const source = fenceSource(child);

    // Lua is the only language worth colouring here: it is what the
    // posts are about, and the highlighter already exists for the
    // landing's samples. Everything else keeps its shell transcript
    // shape rather than being coloured wrongly.
    if (language === 'lua') {
      return (
        <pre className={classes.block}>
          <code>{highlightLua(source)}</code>
        </pre>
      );
    }
    return (
      <pre className={language === 'sql' ? classes.block : classes.shell}>
        {source}
      </pre>
    );
  },
  h1: heading('h2', classes.h2),
  h2: heading('h2', classes.h2),
  h3: heading('h3', classes.h3),
  h4: heading('h4', classes.h4),
  p: (props: object) => <p className={classes.paragraph} {...props} />,
  ul: (props: object) => <ul className={classes.list} {...props} />,
  ol: (props: object) => <ol className={classes.list} {...props} />,
  blockquote: (props: object) => (
    <blockquote className={classes.quote} {...props} />
  ),
};

/** The post's date as the list and the header both show it. */
function published(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : date.toISOString().slice(0, 10);
}

export default function PostPage({ post }: { post: MDXPost }) {
  return (
    <article className={classes.article}>
      <NextSeo title={post.meta.title} description={post.meta.excerpt} />
      <Link href="/blog" className={classes.back}>
        <Icon name="arrowLeft" size={12} />
        blog
      </Link>
      <h1 className={classes.title}>{post.meta.title}</h1>
      <div className={classes.meta}>
        {post.meta.category && (
          <span className={classes.category}>{post.meta.category}</span>
        )}
        <span>{published(post.meta.date)}</span>
        {post.meta.tags.map((tag) => (
          <span key={tag}>#{tag}</span>
        ))}
      </div>
      <hr className={classes.rule} />
      <MDXRemote {...post.source} components={components} />
    </article>
  );
}

export const getStaticProps: GetStaticProps = async ({ params }) => {
  const { slug } = params as { slug: string };
  const { content, meta } = getPostFromSlug(slug);
  const mdxSource = await serialize(content, {
    mdxOptions: {
      rehypePlugins: [
        rehypeSlug,
        [rehypeAutolinkHeadings, { behavior: 'wrap' }],
      ],
    },
  });

  return { props: { post: { source: mdxSource, meta } } };
};

export const getStaticPaths: GetStaticPaths = async () => {
  const paths = getSlugs().map((slug) => ({ params: { slug } }));

  return {
    paths,
    fallback: false,
  };
};
