import fs from 'fs';
import { sync } from 'glob';
import matter from 'gray-matter';
import normalize from 'normalize-path';
import path from 'path';

export interface PostMeta {
  excerpt: string;
  slug: string;
  title: string;
  tags: string[];
  date: string;
  category: string;
  image?: string;
}

interface Post {
  content: string;
  meta: PostMeta;
}

/* Joining the current working directory with the `posts` directory. */
const POSTS_PATH = path.join(process.cwd(), 'src/posts');

/**
 * It takes all the files in the `posts` directory, splits the file path into an array, grabs the last
 * item in the array (the file name), splits the file name into an array, grabs the first item in the
 * array (the slug), and returns an array of slugs
 * @returns An array of strings.
 */
export const getSlugs = (): string[] => {
  const paths = sync(normalize(`${POSTS_PATH}/*.mdx`));

  return paths.map((filePath) => {
    const parts = filePath.split('/');
    const fileName = parts[parts.length - 1];
    const [slug] = fileName.split('.');
    return slug;
  });
};

/**
 * It takes a slug, reads the corresponding file, and returns a `Post` object
 * @param {string} slug - The slug of the post we want to get.
 * @returns An object with two properties: content and meta.
 */
export const getPostFromSlug = (slug: string): Post => {
  const postPath = path.join(POSTS_PATH, `${slug}.mdx`);
  const source = fs.readFileSync(postPath);
  const { content, data } = matter(source);

  return {
    content,
    meta: {
      slug,
      excerpt: data.excerpt ?? '',
      title: data.title ?? slug,
      tags: (data.tags ?? []).sort(),
      date: (data.date ?? new Date()).toString(),
      image: data.image ?? undefined,
      category: data.category ?? undefined,
    },
  };
};

/**
 * Get all the slugs, get the post from each slug, sort the posts by date, and return the posts
 * @returns An array of objects.
 */
export const getAllPosts = () => {
  const posts = getSlugs()
    .map((slug) => getPostFromSlug(slug))
    .sort((a, b) => {
      if (a.meta.date > b.meta.date) return 1;
      if (a.meta.date < b.meta.date) return -1;
      return 0;
    })
    .reverse();
  return posts;
};
