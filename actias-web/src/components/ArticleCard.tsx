import React from 'react';

import Image from 'next/image';
import Link from 'next/link';

import { Anchor, Badge, Box, Card, CardSection, Text } from '@mantine/core';

export interface ArticleCardProps {
  image?: string;
  link: string;
  title: string;
  description: string;
  category?: string;
}

export default function ArticleCard({
  image,
  link,
  title,
  description,
  category,
}: ArticleCardProps &
  Omit<React.ComponentPropsWithoutRef<'div'>, keyof ArticleCardProps>) {
  return (
    <Card withBorder shadow="sm" padding="lg" radius="md">
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
                objectFit="cover"
                alt={`${title} cover image`}
                src={image}
                layout="fill"
                sizes="50vw"
                priority
              />
            </Anchor>
          </Link>
        </CardSection>
      )}

      <Box mt="10px">
        {category && (
          <>
            <Badge variant="filled">{category}</Badge>
            <br />
          </>
        )}

        <Link href={link} passHref>
          <Text fw={500} component="a">
            {title}
          </Text>
        </Link>

        <Text size="sm" c="dimmed" lineClamp={4}>
          {description}
        </Text>
      </Box>
    </Card>
  );
}
