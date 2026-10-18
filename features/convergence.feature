Feature: Convergence
  Every replica that has received the same updates holds the same document,
  whatever order the updates arrived in and however often.

  @vector:concurrent-attr-same-key
  Scenario: Two people set the same attribute at once
    Given a circle
    When Ann sets its fill to red and Ben sets it to blue at the same time
    Then everyone sees the same fill, and the higher client ID wins

  @vector:concurrent-attr-different-keys
  Scenario: Two people set different attributes at once
    Given a circle
    When Ann sets its fill and Ben sets its stroke at the same time
    Then everyone sees both

  @vector:concurrent-insert-same-position
  Scenario: Two people insert at the same position
    Given a drawing with one rectangle
    When Ann adds a circle after it and Ben adds an ellipse after it at the same time
    Then everyone sees both new shapes after the rectangle, the lower client ID first

  @vector:concurrent-insert-index-0
  Scenario: Two people insert a first child at once
    Given a drawing with one rectangle
    When Ann and Ben each insert a shape before it at the same time
    Then everyone sees both new shapes before the rectangle, the lower client ID first

  @vector:edit-inside-deleted
  Scenario: An edit inside an element someone else deleted
    Given a group holding a rectangle
    When Ann deletes the group and Ben recolours the rectangle at the same time
    Then the group is gone for everyone, and Ben's edit isn't shown

  @vector:tspan-whitespace
  Scenario: Whitespace between text spans survives
    Given text with two spans separated by a space
    When Ann edits one span and Ben sets the other's x at the same time
    Then the space between them is still there

  @vector:comment-pi-cdata
  Scenario: Comments, processing instructions and CDATA round-trip
    Given a drawing with a comment before it and a comment, a processing instruction and a CDATA style inside it
    When Ann adds a comment and Ben edits the style
    Then all of them come back intact

  @vector:namespaced-attrs
  Scenario: Inkscape, Sodipodi and XLink attributes
    Given an Inkscape drawing with a named view, a layer and a use element
    When Ann marks the layer and Ben sets the document name
    Then the attributes keep their prefixes and sort by namespace

  @vector:no-namespace-element
  Scenario: An element in no namespace inside SVG
    Given metadata holding an element in no namespace
    When Ann sets an attribute on it
    Then it is still written in no namespace

  @vector:deletes-out-of-order
  Scenario: A delete arrives before the insert it deletes
    Given Ann added a rectangle and a circle
    When Ben deletes the rectangle and Ann recolours the circle
    Then every replica converges, whatever order the updates arrive in
