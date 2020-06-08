/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! https://drafts.csswg.org/css-sizing/

use crate::style_ext::ComputedValuesExt;
use style::properties::longhands::box_sizing::computed_value::T as BoxSizing;
use style::properties::ComputedValues;
use style::values::computed::{Length, LengthPercentage, Percentage};
use style::Zero;

/// Which min/max-content values should be computed during box construction
#[derive(Clone, Copy, Debug)]
pub(crate) enum ContentSizesRequest {
    Inline,
    None,
}

impl ContentSizesRequest {
    pub fn inline_if(condition: bool) -> Self {
        if condition {
            Self::Inline
        } else {
            Self::None
        }
    }

    pub fn requests_inline(self) -> bool {
        match self {
            Self::Inline => true,
            Self::None => false,
        }
    }

    pub fn if_requests_inline<T>(self, f: impl FnOnce() -> T) -> Option<T> {
        match self {
            Self::Inline => Some(f()),
            Self::None => None,
        }
    }

    pub fn compute(self, compute_inline: impl FnOnce() -> ContentSizes) -> BoxContentSizes {
        match self {
            Self::Inline => BoxContentSizes::Inline(compute_inline()),
            Self::None => BoxContentSizes::NoneWereRequested,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ContentSizes {
    pub min_content: Length,
    pub max_content: Length,
}

/// https://drafts.csswg.org/css-sizing/#intrinsic-sizes
impl ContentSizes {
    pub fn zero() -> Self {
        Self {
            min_content: Length::zero(),
            max_content: Length::zero(),
        }
    }

    fn map(&self, f: impl Fn(Length) -> Length) -> Self {
        Self {
            min_content: f(self.min_content),
            max_content: f(self.max_content),
        }
    }

    pub fn max_assign(&mut self, other: &Self) {
        self.min_content.max_assign(other.min_content);
        self.max_content.max_assign(other.max_content);
    }

    /// Relevant to outer intrinsic inline sizes, for percentages from padding and margin.
    pub fn adjust_for_pbm_percentages(&mut self, percentages: Percentage) {
        // " Note that this may yield an infinite result, but undefined results
        //   (zero divided by zero) must be treated as zero. "
        if self.max_content.px() == 0. {
            // Avoid a potential `NaN`.
            // Zero is already the result we want regardless of `denominator`.
        } else {
            let denominator = (1. - percentages.0).max(0.);
            self.max_content = Length::new(self.max_content.px() / denominator);
        }
    }
}

/// Optional min/max-content for storage in the box tree
#[derive(Debug, Serialize)]
pub(crate) enum BoxContentSizes {
    NoneWereRequested, // … during box construction
    Inline(ContentSizes),
}

impl BoxContentSizes {
    fn expect_inline(&self) -> &ContentSizes {
        match self {
            Self::NoneWereRequested => panic!("Accessing content size that was not requested"),
            Self::Inline(s) => s,
        }
    }

    /// https://dbaron.org/css/intrinsic/#outer-intrinsic
    pub fn outer_inline(&self, style: &ComputedValues) -> ContentSizes {
        let (mut outer, percentages) = self.outer_inline_and_percentages(style);
        outer.adjust_for_pbm_percentages(percentages);
        outer
    }

    pub(crate) fn outer_inline_and_percentages(
        &self,
        style: &ComputedValues,
    ) -> (ContentSizes, Percentage) {
        let padding = style.padding();
        let border = style.border_width();
        let margin = style.margin();

        let mut pbm_percentages = Percentage::zero();
        let mut decompose = |x: &LengthPercentage| {
            pbm_percentages += x.to_percentage().unwrap_or_else(Zero::zero);
            x.to_length().unwrap_or_else(Zero::zero)
        };
        let pb_lengths =
            border.inline_sum() + decompose(padding.inline_start) + decompose(padding.inline_end);
        let mut m_lengths = Length::zero();
        if let Some(m) = margin.inline_start.non_auto() {
            m_lengths += decompose(m)
        }
        if let Some(m) = margin.inline_end.non_auto() {
            m_lengths += decompose(m)
        }

        let box_sizing = style.get_position().box_sizing;
        let inline_size = style
            .box_size()
            .inline
            .non_auto()
            // Percentages for 'width' are treated as 'auto'
            .and_then(|lp| lp.to_length());
        let min_inline_size = style
            .min_box_size()
            .inline
            // Percentages for 'min-width' are treated as zero
            .percentage_relative_to(Length::zero())
            // FIXME: 'auto' is not zero in Flexbox
            .auto_is(Length::zero);
        let max_inline_size = style
            .max_box_size()
            .inline
            // Percentages for 'max-width' are treated as 'none'
            .and_then(|lp| lp.to_length());
        let clamp = |l: Length| l.clamp_between_extremums(min_inline_size, max_inline_size);

        let border_box_sizes = match inline_size {
            Some(non_auto) => {
                let clamped = clamp(non_auto);
                let border_box_size = match box_sizing {
                    BoxSizing::ContentBox => clamped + pb_lengths,
                    BoxSizing::BorderBox => clamped,
                };
                ContentSizes {
                    min_content: border_box_size,
                    max_content: border_box_size,
                }
            },
            None => self.expect_inline().map(|content_box_size| {
                match box_sizing {
                    // Clamp to 'min-width' and 'max-width', which are sizing the…
                    BoxSizing::ContentBox => clamp(content_box_size) + pb_lengths,
                    BoxSizing::BorderBox => clamp(content_box_size + pb_lengths),
                }
            }),
        };

        let outer = border_box_sizes.map(|s| s + m_lengths);
        (outer, pbm_percentages)
    }

    /// https://drafts.csswg.org/css2/visudet.html#shrink-to-fit-float
    pub(crate) fn shrink_to_fit(&self, available_size: Length) -> Length {
        let inline = self.expect_inline();
        available_size
            .max(inline.min_content)
            .min(inline.max_content)
    }
}
