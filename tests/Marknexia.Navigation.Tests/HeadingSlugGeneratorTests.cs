using FluentAssertions;
using Marknexia.Core;
using Xunit;

namespace Marknexia.Navigation.Tests;

public class HeadingSlugGeneratorTests
{
    [Fact]
    public void GenerateSlug_BasicText_GeneratesLowercaseHyphenatedSlug()
    {
        var generator = new HeadingSlugGenerator();
        string slug = generator.GenerateSlug("Database Architecture");
        slug.Should().Be("database-architecture");
    }

    [Fact]
    public void GenerateSlug_DuplicateHeadings_AppendsDeterministicCounter()
    {
        var generator = new HeadingSlugGenerator();
        string slug1 = generator.GenerateSlug("Section");
        string slug2 = generator.GenerateSlug("Section");
        string slug3 = generator.GenerateSlug("Section");

        slug1.Should().Be("section");
        slug2.Should().Be("section-1");
        slug3.Should().Be("section-2");
    }

    [Fact]
    public void GenerateSlug_SpecialCharactersAndFormatting_StripsPunctuation()
    {
        var generator = new HeadingSlugGenerator();
        string slug = generator.GenerateSlug("Section 1.2: **Bold** & `Code` (#testing!)");
        slug.Should().Be("section-12-bold-code-testing");
    }

    [Fact]
    public void GenerateSlug_HtmlTags_StripsHtmlTags()
    {
        var generator = new HeadingSlugGenerator();
        string slug = generator.GenerateSlug("Custom <code>API</code> Method");
        slug.Should().Be("custom-api-method");
    }
}
