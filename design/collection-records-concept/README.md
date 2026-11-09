# Ranked song records · concept

This is a visual proposal only. It uses fictional songs and sample counts; it does
not change the app or the user's listening history.

## Proposed ladder

| Tier | Full listens |
| --- | ---: |
| Iron | 10 |
| Bronze | 25 |
| Silver | 50 |
| Gold | 100 |
| Platinum | 250 |
| Double Platinum | 500 |
| Diamond | 1,000 |

The display sorts songs by full listens. A full listen retains Moosik's existing
90% distinct-coverage rule. Listening time is a separate total and includes partial
plays. The top three get tier-specific 3D displays; the rest continue in ranked
rows. Double Platinum gets a violet display; Diamond gets the faceted blue
centerpiece and strongest glints. These are personal goals, not sales awards.

The mockup is responsive: the featured cards stack at narrow width. The source is
[index.html](index.html), with rendered [desktop](desktop.png) and
[narrow](narrow.png) previews.

## Implemented app previews

The native Collection is shown in [desktop](implemented-desktop-qa.png) and
[narrow](implemented-narrow-qa.png) signed-app QA captures. The
[player preview](implemented-player-qa.png) shows the colored rank sparkle on
the lower-left of the now-playing cover. These captures use a synthetic listening
profile and sample artwork; they contain no personal listening history.

The transparent hero asset [diamond-trophy.png](diamond-trophy.png) was generated
with the built-in image tool using this prompt:

> Use case: stylized-concept. Asset type: a single transparent decorative 3D song trophy object for a dark desktop music app Collection mockup. Subject: a square album artwork slab, upright and slightly turned in three-quarter perspective, with a thick glossy edge. The front artwork is an original minimal architectural photo-like composition of pale stairs and concrete geometry, no people. Directly beneath the slab is a faceted translucent icy-blue diamond pedestal and subtle glass plinth, integrated as one collectible object. A few small crisp star-shaped light glints near the diamond, restrained rather than cartoonish. Premium product render, precise geometry, dark ink shadows and soft cool rim light, designed to read at card size. Composition: centered full object with breathing room, front cover mostly visible, no crop. Transparent background with genuine alpha. No words, letters, numbers, logo, app chrome, mockup frame, watermark, or other items.
