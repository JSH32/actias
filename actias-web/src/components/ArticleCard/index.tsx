import React from 'react';

import Image from 'next/image';
import Link from 'next/link';

import { Anchor, Badge, Card, CardSection, Text } from '@mantine/core';

import useStyles from './styles';

export interface ArticleCardProps {
  image?: string;
  link: string;
  title: string;
  description: string;
  category?: string;
}

export default function ArticleCard({
  className,
  image,
  link,
  title,
  description,
  category,
}: ArticleCardProps &
  Omit<React.ComponentPropsWithoutRef<'div'>, keyof ArticleCardProps>) {
  const { classes, cx } = useStyles();

  return (
    <Card withBorder radius="md" className={cx(classes.card, className)}>
      {image && (
        <CardSection
          style={{
            position: 'relative',
            minHeight: 180,
          }}
        >
          <Link href={link} passHref>
            <Anchor component="a">
              <Image
                alt={`${title} cover image`}
                src={image}
                layout="fill"
                objectFit="cover"
                sizes="50vw"
                priority
              />
            </Anchor>
          </Link>
        </CardSection>
      )}

      {category && (
        <Badge className={classes.category} variant="filled">
          {category}
        </Badge>
      )}

      <Link href={link} passHref>
        <Text className={classes.title} weight={500} component="a">
          {title}
        </Text>
      </Link>

      <Text size="sm" color="dimmed" lineClamp={4}>
        {description}
      </Text>
    </Card>
  );
}
